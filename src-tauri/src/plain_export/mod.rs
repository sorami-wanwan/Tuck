mod disk;
mod naming;
mod plan;
#[cfg(test)]
mod tests;
mod zip_stream;

pub use disk::{export_plain_batch, export_plain_item};
pub use naming::{build_item_filename, content_disposition_attachment};
pub use plan::build_plain_export_plan;
pub use zip_stream::{
    compute_zip_archive_size, stream_single_item_to_channel, stream_zip_plan_to_channel,
};

pub(super) fn unix_secs_to_utc_ymdhms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hh = (rem / 3600) as u32;
    let mm = ((rem % 3600) / 60) as u32;
    let ss = (rem % 60) as u32;

    // Howard Hinnant's civil-from-days algorithm
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }).max(1980) as u32;
    (year, m as u32, d as u32, hh, mm, ss)
}
