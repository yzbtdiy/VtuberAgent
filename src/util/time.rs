use chrono::{DateTime, FixedOffset, Utc};

const BEIJING_OFFSET_SECONDS: i32 = 8 * 3600;

fn beijing_offset() -> FixedOffset {
    FixedOffset::east_opt(BEIJING_OFFSET_SECONDS)
        .expect("UTC+8 offset should be available for Beijing time")
}

pub fn now_in_beijing() -> DateTime<FixedOffset> {
    Utc::now().with_timezone(&beijing_offset())
}

pub fn format_beijing(now: &DateTime<FixedOffset>, pattern: &str) -> String {
    now.format(pattern).to_string()
}

pub fn beijing_rfc3339(now: &DateTime<FixedOffset>) -> String {
    now.to_rfc3339()
}
