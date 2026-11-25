//! Minimal pure-Rust ZIP reader & writer backed by existing `flate2`.
//!
//! Supports Stored (method 0) and Deflate (method 8) with IEEE 802.3 CRC-32,
//! used by both the Portable EPUB Reader (`src/epub.rs`) and EPUB Exporter (`src/epub_export.rs`).

use anyhow::{Result, bail};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::collections::HashMap;
use std::io::{Read, Write};

const LOCAL_FILE_HEADER_SIG: u32 = 0x0403_4b50;
const CENTRAL_DIR_HEADER_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;

pub(crate) fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[inline]
fn read_u16_le(data: &[u8], off: usize) -> Result<u16> {
    let slice = data
        .get(off..off + 2)
        .ok_or_else(|| anyhow::anyhow!("Unexpected EOF reading u16"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

#[inline]
fn read_u32_le(data: &[u8], off: usize) -> Result<u32> {
    let slice = data
        .get(off..off + 4)
        .ok_or_else(|| anyhow::anyhow!("Unexpected EOF reading u32"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[derive(Clone, Debug)]
struct ZipEntryMeta {
    method: u16,
    comp_size: u32,
    uncomp_size: u32,
    local_header_offset: u32,
}

pub(crate) struct ZipReader<'a> {
    data: &'a [u8],
    entries: HashMap<String, ZipEntryMeta>,
    order: Vec<String>,
}

impl<'a> ZipReader<'a> {
    pub(crate) fn from_bytes(data: &'a [u8]) -> Result<Self> {
        if data.len() < 22 {
            bail!("File is too small to be a ZIP/EPUB archive");
        }
        let search_start = data.len().saturating_sub(65_557);
        let mut eocd_off = None;
        for i in (search_start..=data.len() - 22).rev() {
            if data[i..i + 4] == EOCD_SIG.to_le_bytes() {
                let comment_len = read_u16_le(data, i + 20)? as usize;
                if i + 22 + comment_len <= data.len() {
                    eocd_off = Some(i);
                    break;
                }
            }
        }
        let eocd = eocd_off.ok_or_else(|| anyhow::anyhow!("Invalid ZIP/EPUB: EOCD record not found"))?;
        let cd_size = read_u32_le(data, eocd + 12)? as usize;
        let cd_off = read_u32_le(data, eocd + 16)? as usize;
        let cd_end = cd_off
            .checked_add(cd_size)
            .ok_or_else(|| anyhow::anyhow!("Invalid ZIP central directory bounds"))?;
        if cd_end > data.len() {
            bail!("ZIP central directory extends past end of file");
        }

        let mut entries = HashMap::new();
        let mut order = Vec::new();
        let mut pos = cd_off;
        while pos + 46 <= cd_end {
            if read_u32_le(data, pos)? != CENTRAL_DIR_HEADER_SIG {
                break;
            }
            let method = read_u16_le(data, pos + 10)?;
            let comp_size = read_u32_le(data, pos + 20)?;
            let uncomp_size = read_u32_le(data, pos + 24)?;
            let name_len = read_u16_le(data, pos + 28)? as usize;
            let extra_len = read_u16_le(data, pos + 30)? as usize;
            let comment_len = read_u16_le(data, pos + 32)? as usize;
            let local_header_offset = read_u32_le(data, pos + 42)?;

            let name_start = pos + 46;
            let name_end = name_start + name_len;
            if name_end > cd_end {
                bail!("ZIP entry name exceeds central directory");
            }
            let name = String::from_utf8_lossy(&data[name_start..name_end]).to_string();
            entries.insert(
                name.clone(),
                ZipEntryMeta {
                    method,
                    comp_size,
                    uncomp_size,
                    local_header_offset,
                },
            );
            order.push(name);
            pos = name_end + extra_len + comment_len;
        }

        Ok(Self {
            data,
            entries,
            order,
        })
    }

    pub(crate) fn read_file(&self, name: &str) -> Result<Vec<u8>> {
        let norm = name.trim_start_matches('/');
        let meta = self
            .entries
            .get(norm)
            .or_else(|| {
                self.order
                    .iter()
                    .find(|k| k.eq_ignore_ascii_case(norm))
                    .and_then(|k| self.entries.get(k))
            })
            .ok_or_else(|| anyhow::anyhow!("EPUB entry not found: {norm}"))?;

        let lh = meta.local_header_offset as usize;
        if lh + 30 > self.data.len() || read_u32_le(self.data, lh)? != LOCAL_FILE_HEADER_SIG {
            bail!("Invalid ZIP local file header for {norm}");
        }
        let name_len = read_u16_le(self.data, lh + 26)? as usize;
        let extra_len = read_u16_le(self.data, lh + 28)? as usize;
        let data_start = lh + 30 + name_len + extra_len;
        let data_end = data_start
            .checked_add(meta.comp_size as usize)
            .ok_or_else(|| anyhow::anyhow!("ZIP entry overflow for {norm}"))?;
        if data_end > self.data.len() {
            bail!("ZIP entry data truncated for {norm}");
        }
        let raw = &self.data[data_start..data_end];
        match meta.method {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut dec = DeflateDecoder::new(raw);
                let mut out = Vec::with_capacity(meta.uncomp_size as usize);
                dec.read_to_end(&mut out)?;
                Ok(out)
            }
            other => bail!("Unsupported ZIP compression method {other} for {norm}"),
        }
    }
}

struct WrittenEntry {
    name: Vec<u8>,
    method: u16,
    crc: u32,
    comp_size: u32,
    uncomp_size: u32,
    local_offset: u32,
}

pub(crate) struct ZipWriter {
    buf: Vec<u8>,
    entries: Vec<WrittenEntry>,
}

impl ZipWriter {
    pub(crate) fn new() -> Self {
        Self {
            buf: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub(crate) fn add_stored(&mut self, name: &str, data: &[u8]) {
        self.write_entry(name, data, 0, data);
    }

    pub(crate) fn add_deflated(&mut self, name: &str, data: &[u8]) {
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        let _ = enc.write_all(data);
        let compressed = enc.finish().unwrap_or_else(|_| data.to_vec());
        if compressed.len() < data.len() {
            self.write_entry(name, data, 8, &compressed);
        } else {
            self.write_entry(name, data, 0, data);
        }
    }

    fn write_entry(&mut self, name: &str, uncomp: &[u8], method: u16, payload: &[u8]) {
        let local_offset = self.buf.len() as u32;
        let crc = crc32_ieee(uncomp);
        let name_bytes = name.as_bytes();
        let comp_size = payload.len() as u32;
        let uncomp_size = uncomp.len() as u32;

        // Local file header
        self.buf.extend_from_slice(&LOCAL_FILE_HEADER_SIG.to_le_bytes());
        self.buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
        self.buf.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
        self.buf.extend_from_slice(&method.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
        self.buf.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
        self.buf.extend_from_slice(&crc.to_le_bytes());
        self.buf.extend_from_slice(&comp_size.to_le_bytes());
        self.buf.extend_from_slice(&uncomp_size.to_le_bytes());
        self.buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // extra len
        self.buf.extend_from_slice(name_bytes);
        self.buf.extend_from_slice(payload);

        self.entries.push(WrittenEntry {
            name: name_bytes.to_vec(),
            method,
            crc,
            comp_size,
            uncomp_size,
            local_offset,
        });
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.buf.len() as u32;
        for e in &self.entries {
            self.buf.extend_from_slice(&CENTRAL_DIR_HEADER_SIG.to_le_bytes());
            self.buf.extend_from_slice(&20u16.to_le_bytes()); // version made by
            self.buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
            self.buf.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
            self.buf.extend_from_slice(&e.method.to_le_bytes());
            self.buf.extend_from_slice(&0u16.to_le_bytes());
            self.buf.extend_from_slice(&0x5421u16.to_le_bytes());
            self.buf.extend_from_slice(&e.crc.to_le_bytes());
            self.buf.extend_from_slice(&e.comp_size.to_le_bytes());
            self.buf.extend_from_slice(&e.uncomp_size.to_le_bytes());
            self.buf.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // extra
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // comment
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // disk
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // int attrs
            self.buf.extend_from_slice(&0u32.to_le_bytes()); // ext attrs
            self.buf.extend_from_slice(&e.local_offset.to_le_bytes());
            self.buf.extend_from_slice(&e.name);
        }
        let cd_size = (self.buf.len() as u32) - cd_offset;
        let count = self.entries.len() as u16;

        // End of Central Directory
        self.buf.extend_from_slice(&EOCD_SIG.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(&count.to_le_bytes());
        self.buf.extend_from_slice(&count.to_le_bytes());
        self.buf.extend_from_slice(&cd_size.to_le_bytes());
        self.buf.extend_from_slice(&cd_offset.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zip_mini_stored_and_deflated_roundtrip() {
        let mut zw = ZipWriter::new();
        zw.add_stored("mimetype", b"application/epub+zip");
        let long_text = "Hello EPUB world! ".repeat(40);
        zw.add_deflated("OEBPS/ch1.xhtml", long_text.as_bytes());
        let bytes = zw.finish();

        let zr = ZipReader::from_bytes(&bytes).expect("parse zip");
        assert_eq!(zr.read_file("mimetype").unwrap(), b"application/epub+zip");
        assert_eq!(zr.read_file("OEBPS/ch1.xhtml").unwrap(), long_text.as_bytes());
    }
}
