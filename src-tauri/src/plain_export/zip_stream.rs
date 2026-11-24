use std::io;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::db::{now_ms, ExportItemMeta, VaultDb};

use super::plan::PlainExportPlan;
use super::unix_secs_to_utc_ymdhms;

const STREAM_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub(super) const ZIP_GPBF_DATA_DESC_UTF8: u16 = 0x0808;
const ZIP_METHOD_STORE: u16 = 0;
const ZIP_VERSION_20: u16 = 20;
const ZIP_VERSION_45: u16 = 45;
const ZIP_UNIX_VERSION_MADE_BY: u16 = (3 << 8) | 45;
const ZIP_UNIX_REGULAR_FILE_ATTRS: u32 = 0o100644 << 16;

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            if (c & 1) != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

pub(super) fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = CRC32_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

fn current_dos_time_date() -> (u16, u16) {
    let secs = (now_ms().max(0) as u64) / 1000;
    let (year, month, day, hh, mm, ss) = unix_secs_to_utc_ymdhms(secs);
    let clamped_year = year.clamp(1980, 2107);
    let dos_time = ((hh as u16) << 11) | ((mm as u16) << 5) | ((ss / 2) as u16);
    let dos_date = (((clamped_year - 1980) as u16) << 9) | ((month as u16) << 5) | (day as u16);
    (dos_time, dos_date)
}

pub fn compute_zip_archive_size(plan: &PlainExportPlan) -> u64 {
    let mut offset = 0u64;
    let mut cd_size = 0u64;

    for entry in &plan.files {
        let name_len = entry.zip_entry_name().len() as u64;
        let need_size64 = entry.byte_size >= u64::from(u32::MAX);
        let lfh_extra_len = if need_size64 { 20u64 } else { 0u64 };
        let dd_len = if need_size64 { 24u64 } else { 16u64 };

        let local_offset = offset;
        offset = offset
            .saturating_add(30)
            .saturating_add(name_len)
            .saturating_add(lfh_extra_len)
            .saturating_add(entry.byte_size)
            .saturating_add(dd_len);

        let need_offset64 = local_offset >= u64::from(u32::MAX);
        let cd_extra_len = if need_size64 || need_offset64 {
            4 + if need_size64 { 16 } else { 0 } + if need_offset64 { 8 } else { 0 }
        } else {
            0u64
        };
        cd_size = cd_size
            .saturating_add(46)
            .saturating_add(name_len)
            .saturating_add(cd_extra_len);
    }

    let need_eocd64 = plan.files.len() >= 0xFFFF
        || cd_size >= u64::from(u32::MAX)
        || offset >= u64::from(u32::MAX);
    let eocd64_overhead = if need_eocd64 { 56 + 20 } else { 0 };
    offset
        .saturating_add(cd_size)
        .saturating_add(eocd64_overhead)
        .saturating_add(22)
}

struct ZipCdRecord {
    name: Vec<u8>,
    crc32: u32,
    size: u64,
    local_header_offset: u64,
    need_size64: bool,
}

pub struct ZipStoreStreamWriter<F> {
    emit: F,
    dos_time: u16,
    dos_date: u16,
    offset: u64,
    cd_records: Vec<ZipCdRecord>,
}

impl<F> ZipStoreStreamWriter<F>
where
    F: FnMut(Bytes) -> Result<(), io::Error>,
{
    pub fn new(emit: F) -> Self {
        let (dos_time, dos_date) = current_dos_time_date();
        Self {
            emit,
            dos_time,
            dos_date,
            offset: 0,
            cd_records: Vec::new(),
        }
    }

    fn write_bytes(&mut self, buf: Bytes) -> Result<(), io::Error> {
        self.offset = self.offset.saturating_add(buf.len() as u64);
        (self.emit)(buf)
    }

    pub fn write_entry_header(
        &mut self,
        name_str: &str,
        expected_size: u64,
    ) -> Result<(), io::Error> {
        let name = name_str.as_bytes();
        let need_size64 = expected_size >= u64::from(u32::MAX);
        let version_needed = if need_size64 {
            ZIP_VERSION_45
        } else {
            ZIP_VERSION_20
        };
        let extra_len: u16 = if need_size64 { 20 } else { 0 };

        let mut hdr = Vec::with_capacity(30 + name.len() + usize::from(extra_len));
        hdr.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        hdr.extend_from_slice(&version_needed.to_le_bytes());
        hdr.extend_from_slice(&ZIP_GPBF_DATA_DESC_UTF8.to_le_bytes());
        hdr.extend_from_slice(&ZIP_METHOD_STORE.to_le_bytes());
        hdr.extend_from_slice(&self.dos_time.to_le_bytes());
        hdr.extend_from_slice(&self.dos_date.to_le_bytes());
        hdr.extend_from_slice(&0u32.to_le_bytes()); // CRC32 (in Data Descriptor)
        hdr.extend_from_slice(&0u32.to_le_bytes()); // Compressed size (in Data Descriptor)
        hdr.extend_from_slice(&0u32.to_le_bytes()); // Uncompressed size (in Data Descriptor)
        hdr.extend_from_slice(&(name.len() as u16).to_le_bytes());
        hdr.extend_from_slice(&extra_len.to_le_bytes());
        hdr.extend_from_slice(name);
        if need_size64 {
            hdr.extend_from_slice(&0x0001u16.to_le_bytes());
            hdr.extend_from_slice(&16u16.to_le_bytes());
            hdr.extend_from_slice(&0u64.to_le_bytes());
            hdr.extend_from_slice(&0u64.to_le_bytes());
        }

        let local_header_offset = self.offset;
        self.cd_records.push(ZipCdRecord {
            name: name.to_vec(),
            crc32: 0,
            size: 0,
            local_header_offset,
            need_size64,
        });
        self.write_bytes(Bytes::from(hdr))
    }

    pub fn write_entry_data(&mut self, chunk: Bytes) -> Result<(), io::Error> {
        if chunk.is_empty() {
            return Ok(());
        }
        if let Some(rec) = self.cd_records.last_mut() {
            rec.crc32 = crc32_update(rec.crc32, &chunk);
            rec.size = rec.size.saturating_add(chunk.len() as u64);
        }
        self.write_bytes(chunk)
    }

    pub fn finish_entry(&mut self) -> Result<(), io::Error> {
        let (crc32, size, need_size64) = self
            .cd_records
            .last()
            .map(|r| (r.crc32, r.size, r.need_size64))
            .ok_or_else(|| io::Error::other("no active zip entry"))?;

        let mut dd = Vec::with_capacity(if need_size64 { 24 } else { 16 });
        dd.extend_from_slice(&0x0807_4B50u32.to_le_bytes());
        dd.extend_from_slice(&crc32.to_le_bytes());
        if need_size64 {
            dd.extend_from_slice(&size.to_le_bytes());
            dd.extend_from_slice(&size.to_le_bytes());
        } else {
            let s32 = size as u32;
            dd.extend_from_slice(&s32.to_le_bytes());
            dd.extend_from_slice(&s32.to_le_bytes());
        }
        self.write_bytes(Bytes::from(dd))
    }

    pub fn finish_archive(mut self) -> Result<(), io::Error> {
        let cd_offset = self.offset;
        let entry_count = self.cd_records.len();
        let mut cd_buf = Vec::new();

        for rec in &self.cd_records {
            let need_offset64 = rec.local_header_offset >= u64::from(u32::MAX);
            let need_zip64 = rec.need_size64 || need_offset64;
            let version_needed = if need_zip64 {
                ZIP_VERSION_45
            } else {
                ZIP_VERSION_20
            };

            let extra_payload_len: u16 =
                (if rec.need_size64 { 16 } else { 0 }) + (if need_offset64 { 8 } else { 0 });
            let extra_len: u16 = if extra_payload_len > 0 {
                4 + extra_payload_len
            } else {
                0
            };

            let comp_u32 = if rec.need_size64 {
                u32::MAX
            } else {
                rec.size as u32
            };
            let off_u32 = if need_offset64 {
                u32::MAX
            } else {
                rec.local_header_offset as u32
            };

            cd_buf.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_UNIX_VERSION_MADE_BY.to_le_bytes());
            cd_buf.extend_from_slice(&version_needed.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_GPBF_DATA_DESC_UTF8.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_METHOD_STORE.to_le_bytes());
            cd_buf.extend_from_slice(&self.dos_time.to_le_bytes());
            cd_buf.extend_from_slice(&self.dos_date.to_le_bytes());
            cd_buf.extend_from_slice(&rec.crc32.to_le_bytes());
            cd_buf.extend_from_slice(&comp_u32.to_le_bytes());
            cd_buf.extend_from_slice(&comp_u32.to_le_bytes());
            cd_buf.extend_from_slice(&(rec.name.len() as u16).to_le_bytes());
            cd_buf.extend_from_slice(&extra_len.to_le_bytes());
            cd_buf.extend_from_slice(&0u16.to_le_bytes()); // file comment len
            cd_buf.extend_from_slice(&0u16.to_le_bytes()); // disk number start
            cd_buf.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            cd_buf.extend_from_slice(&ZIP_UNIX_REGULAR_FILE_ATTRS.to_le_bytes());
            cd_buf.extend_from_slice(&off_u32.to_le_bytes());
            cd_buf.extend_from_slice(&rec.name);
            if extra_len > 0 {
                cd_buf.extend_from_slice(&0x0001u16.to_le_bytes());
                cd_buf.extend_from_slice(&extra_payload_len.to_le_bytes());
                if rec.need_size64 {
                    cd_buf.extend_from_slice(&rec.size.to_le_bytes());
                    cd_buf.extend_from_slice(&rec.size.to_le_bytes());
                }
                if need_offset64 {
                    cd_buf.extend_from_slice(&rec.local_header_offset.to_le_bytes());
                }
            }
        }

        let cd_size = cd_buf.len() as u64;
        let need_eocd64 = entry_count >= 0xFFFF
            || cd_size >= u64::from(u32::MAX)
            || cd_offset >= u64::from(u32::MAX);

        if need_eocd64 {
            let eocd64_offset = cd_offset.saturating_add(cd_size);
            // ZIP64 End of Central Directory Record (56 bytes)
            cd_buf.extend_from_slice(&0x0606_4B50u32.to_le_bytes());
            cd_buf.extend_from_slice(&44u64.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_UNIX_VERSION_MADE_BY.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_VERSION_45.to_le_bytes());
            cd_buf.extend_from_slice(&0u32.to_le_bytes());
            cd_buf.extend_from_slice(&0u32.to_le_bytes());
            cd_buf.extend_from_slice(&(entry_count as u64).to_le_bytes());
            cd_buf.extend_from_slice(&(entry_count as u64).to_le_bytes());
            cd_buf.extend_from_slice(&cd_size.to_le_bytes());
            cd_buf.extend_from_slice(&cd_offset.to_le_bytes());

            // ZIP64 End of Central Directory Locator (20 bytes)
            cd_buf.extend_from_slice(&0x0706_4B50u32.to_le_bytes());
            cd_buf.extend_from_slice(&0u32.to_le_bytes());
            cd_buf.extend_from_slice(&eocd64_offset.to_le_bytes());
            cd_buf.extend_from_slice(&1u32.to_le_bytes());
        }

        let entries_u16 = entry_count.min(0xFFFF) as u16;
        let cd_size_u32 = cd_size.min(u64::from(u32::MAX)) as u32;
        let cd_offset_u32 = cd_offset.min(u64::from(u32::MAX)) as u32;

        // Standard End of Central Directory Record (22 bytes)
        cd_buf.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        cd_buf.extend_from_slice(&0u16.to_le_bytes());
        cd_buf.extend_from_slice(&0u16.to_le_bytes());
        cd_buf.extend_from_slice(&entries_u16.to_le_bytes());
        cd_buf.extend_from_slice(&entries_u16.to_le_bytes());
        cd_buf.extend_from_slice(&cd_size_u32.to_le_bytes());
        cd_buf.extend_from_slice(&cd_offset_u32.to_le_bytes());
        cd_buf.extend_from_slice(&0u16.to_le_bytes());

        self.write_bytes(Bytes::from(cd_buf))
    }
}

fn pump_item_chunks<F>(
    db_arc: &Arc<Mutex<Option<VaultDb>>>,
    item_id: &str,
    byte_size: u64,
    chunk_size: usize,
    mut on_chunk: F,
) -> Result<(), io::Error>
where
    F: FnMut(Bytes) -> Result<(), io::Error>,
{
    if chunk_size == 0 {
        let bytes = {
            let guard = db_arc
                .lock()
                .map_err(|_| io::Error::other("vault lock poisoned"))?;
            let db = guard
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "vault locked"))?;
            db.get_blob_bytes(item_id)
                .map_err(|e| io::Error::other(e.to_string()))?
        };
        if bytes.len() as u64 != byte_size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "media data truncated",
            ));
        }
        if !bytes.is_empty() {
            on_chunk(Bytes::from(bytes))?;
        }
        return Ok(());
    }

    let total = usize::try_from(byte_size)
        .map_err(|_| io::Error::other("item exceeds addressable range"))?;
    let mut offset = 0usize;
    while offset < total {
        let end = offset
            .saturating_add(STREAM_CHUNK_BYTES)
            .min(total)
            .saturating_sub(1);
        let chunk = {
            let guard = db_arc
                .lock()
                .map_err(|_| io::Error::other("vault lock poisoned"))?;
            let db = guard
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "vault locked"))?;
            db.read_media_range(item_id, offset, end, chunk_size)
                .map_err(|e| io::Error::other(e.to_string()))?
        };
        if chunk.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "media data truncated",
            ));
        }
        let read_len = chunk.len();
        on_chunk(Bytes::from(chunk))?;
        offset = offset.saturating_add(read_len);
    }
    Ok(())
}

pub fn stream_single_item_to_channel(
    db_arc: Arc<Mutex<Option<VaultDb>>>,
    meta: ExportItemMeta,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    let res = pump_item_chunks(
        &db_arc,
        &meta.id,
        meta.byte_size,
        meta.chunk_size,
        |chunk| {
            tx.blocking_send(Ok(chunk))
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected"))
        },
    );
    if let Err(err) = res {
        let _ = tx.blocking_send(Err(err));
    }
}

pub fn stream_zip_plan_to_channel(
    db_arc: Arc<Mutex<Option<VaultDb>>>,
    plan: PlainExportPlan,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    let res = (|| -> Result<(), io::Error> {
        let mut zip = ZipStoreStreamWriter::new(|chunk| {
            tx.blocking_send(Ok(chunk))
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected"))
        });
        for file in &plan.files {
            let entry_name = file.zip_entry_name();
            zip.write_entry_header(&entry_name, file.byte_size)?;
            pump_item_chunks(
                &db_arc,
                &file.item_id,
                file.byte_size,
                file.chunk_size,
                |chunk| zip.write_entry_data(chunk),
            )?;
            zip.finish_entry()?;
        }
        zip.finish_archive()?;
        Ok(())
    })();

    if let Err(err) = res {
        let _ = tx.blocking_send(Err(err));
    }
}
