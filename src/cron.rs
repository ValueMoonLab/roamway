use crate::types::civil_from_days;

/// 5 段 cron（分 时 日 月 周），字段用 Vec<bool> 表示允许值。
/// 日与周同时受限时按标准 cron 的“或”语义处理。
#[derive(Clone, Debug)]
pub struct CronExpr {
    minute: Vec<bool>,
    hour: Vec<bool>,
    day: Vec<bool>,
    month: Vec<bool>,
    weekday: Vec<bool>,
    day_star: bool,
    weekday_star: bool,
}

pub fn parse_cron(expr: &str) -> Result<CronExpr, String> {
    let expr = match expr.trim() {
        "@hourly" => "0 * * * *",
        "@daily" => "0 0 * * *",
        "@weekly" => "0 0 * * 0",
        "@monthly" => "0 0 1 * *",
        other => other,
    };
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(format!(
            "cron must have 5 fields, got {}: '{}'",
            parts.len(),
            expr
        ));
    }
    let minute = parse_field(parts[0], 0, 59)?;
    let hour = parse_field(parts[1], 0, 23)?;
    let day = parse_field(parts[2], 1, 31)?;
    let month = parse_field(parts[3], 1, 12)?;
    let mut weekday = parse_field(parts[4], 0, 7)?;
    if *weekday.last().unwrap_or(&false) {
        weekday[0] = true;
    }
    Ok(CronExpr {
        minute,
        hour,
        day,
        month,
        weekday,
        day_star: parts[2].trim() == "*",
        weekday_star: parts[4].trim() == "*",
    })
}

fn parse_field(field: &str, min: u32, max: u32) -> Result<Vec<bool>, String> {
    let mut bits = vec![false; (max + 1) as usize];
    for item in field.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return Err(format!("empty item in cron field '{}'", field));
        }
        let (base, step) = match item.split_once('/') {
            Some((l, r)) => {
                let step: u32 = r
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad step in '{}'", item))?;
                if step == 0 {
                    return Err(format!("step must be > 0 in '{}'", item));
                }
                (l.trim(), step)
            }
            None => (item, 1),
        };
        let (a, b) = if base == "*" {
            (min, max)
        } else if let Some((l, r)) = base.split_once('-') {
            let a: u32 = l
                .trim()
                .parse()
                .map_err(|_| format!("bad value in '{}'", item))?;
            let b: u32 = r
                .trim()
                .parse()
                .map_err(|_| format!("bad value in '{}'", item))?;
            (a, b)
        } else {
            let n: u32 = base
                .parse()
                .map_err(|_| format!("bad value in '{}'", item))?;
            (n, n)
        };
        if a < min || b > max || a > b {
            return Err(format!("range {}..{} out of bounds in '{}'", a, b, item));
        }
        let mut v = a;
        while v <= b {
            bits[v as usize] = true;
            let next = v.saturating_add(step);
            if next <= v {
                break;
            }
            v = next;
        }
    }
    Ok(bits)
}

impl CronExpr {
    fn matches(&self, minute: u32, hour: u32, day: u32, month: u32, weekday: u32) -> bool {
        if !self.minute.get(minute as usize).copied().unwrap_or(false) {
            return false;
        }
        if !self.hour.get(hour as usize).copied().unwrap_or(false) {
            return false;
        }
        if !self.month.get(month as usize).copied().unwrap_or(false) {
            return false;
        }
        let dom = self.day_star || self.day.get(day as usize).copied().unwrap_or(false);
        let dow = self.weekday_star || self.weekday.get(weekday as usize).copied().unwrap_or(false);
        if self.day_star && self.weekday_star {
            true
        } else if self.day_star {
            dow
        } else if self.weekday_star {
            dom
        } else {
            dom || dow
        }
    }

    /// 从 `now` 之后逐分钟找下一次命中（固定时区偏移），最多向前搜 366 天。
    pub fn next_after(&self, now: i64, tz_offset_secs: i32) -> Option<i64> {
        let off = tz_offset_secs as i64;
        let deadline = now + 366 * 86400;
        let mut local_min = (now + off).div_euclid(60) + 1;
        loop {
            let epoch = local_min * 60 - off;
            if epoch > deadline {
                return None;
            }
            let local_epoch = local_min * 60;
            let days = local_epoch.div_euclid(86400);
            let secs = local_epoch.rem_euclid(86400);
            let (_, m, d) = civil_from_days(days);
            let hour = (secs / 3600) as u32;
            let minute = ((secs % 3600) / 60) as u32;
            let weekday = ((days + 4).rem_euclid(7)) as u32;
            if self.matches(minute, hour, d, m, weekday) {
                return Some(epoch);
            }
            local_min += 1;
        }
    }
}

/// 把时区字符串转换为固定偏移秒数。仅支持 Asia/Shanghai / UTC 与 +HH:MM / -HH:MM。
pub fn timezone_offset_secs(tz: &str) -> Result<i32, String> {
    let tz = tz.trim();
    if tz.is_empty() {
        return Ok(0);
    }
    match tz.to_ascii_lowercase().as_str() {
        "utc" | "z" => return Ok(0),
        "asia/shanghai" => return Ok(8 * 3600),
        _ => {}
    }
    parse_fixed_offset(tz)
}

fn parse_fixed_offset(tz: &str) -> Result<i32, String> {
    let t = tz.trim();
    let (sign, rest) = if let Some(r) = t.strip_prefix('+') {
        (1, r)
    } else if let Some(r) = t.strip_prefix('-') {
        (-1, r)
    } else {
        return Err(format!(
            "timezone '{}' unsupported; use Asia/Shanghai, UTC or a fixed offset like +08:00",
            tz
        ));
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h, m),
        None => (rest, "0"),
    };
    let h: i32 = h
        .trim()
        .parse()
        .map_err(|_| format!("bad timezone offset '{}'", tz))?;
    let m: i32 = m
        .trim()
        .parse()
        .map_err(|_| format!("bad timezone offset '{}'", tz))?;
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return Err(format!("bad timezone offset '{}'", tz));
    }
    Ok(sign * (h * 3600 + m * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    fn epoch(y: i64, m: u32, d: u32, h: u32, min: u32) -> i64 {
        days_from_civil(y, m, d) * 86400 + h as i64 * 3600 + min as i64 * 60
    }

    #[test]
    fn parse_and_match_single_values() {
        let c = parse_cron("30 8 1 1 *").unwrap();
        let t = epoch(2023, 1, 1, 8, 30);
        assert_eq!(c.next_after(t - 60, 0), Some(t));
    }

    #[test]
    fn alias_daily_is_midnight() {
        let c = parse_cron("@daily").unwrap();
        let t = epoch(2023, 1, 2, 0, 0);
        assert_eq!(c.next_after(epoch(2023, 1, 1, 12, 0), 0), Some(t));
    }

    #[test]
    fn next_after_crosses_month() {
        let c = parse_cron("0 0 1 * *").unwrap();
        assert_eq!(
            c.next_after(epoch(2023, 1, 31, 12, 0), 0),
            Some(epoch(2023, 2, 1, 0, 0))
        );
    }

    #[test]
    fn next_after_crosses_year() {
        let c = parse_cron("0 0 1 1 *").unwrap();
        assert_eq!(
            c.next_after(epoch(2022, 12, 31, 12, 0), 0),
            Some(epoch(2023, 1, 1, 0, 0))
        );
    }

    #[test]
    fn next_after_leap_day() {
        let c = parse_cron("0 0 29 2 *").unwrap();
        assert_eq!(
            c.next_after(epoch(2023, 2, 28, 0, 0), 0),
            Some(epoch(2024, 2, 29, 0, 0))
        );
    }

    #[test]
    fn next_after_step_every_five_minutes() {
        let c = parse_cron("*/5 * * * *").unwrap();
        assert_eq!(
            c.next_after(epoch(2023, 1, 1, 0, 1), 0),
            Some(epoch(2023, 1, 1, 0, 5))
        );
    }

    #[test]
    fn next_after_weekday_range_workdays() {
        let c = parse_cron("0 9 * * 1-5").unwrap();
        // 2023-01-01 是周日，下一个工作日是 2023-01-02 09:00
        assert_eq!(
            c.next_after(epoch(2023, 1, 1, 12, 0), 0),
            Some(epoch(2023, 1, 2, 9, 0))
        );
    }

    #[test]
    fn weekday_seven_means_sunday() {
        let c = parse_cron("0 0 * * 7").unwrap();
        // 2023-01-07 是周六，下一个周日是 2023-01-08
        assert_eq!(
            c.next_after(epoch(2023, 1, 7, 12, 0), 0),
            Some(epoch(2023, 1, 8, 0, 0))
        );
    }

    #[test]
    fn next_after_caps_at_366_days() {
        let c = parse_cron("0 0 30 2 *").unwrap();
        assert_eq!(c.next_after(epoch(2023, 2, 28, 0, 0), 0), None);
    }

    #[test]
    fn timezone_offsets_are_parsed() {
        assert_eq!(timezone_offset_secs("Asia/Shanghai").unwrap(), 28800);
        assert_eq!(timezone_offset_secs("UTC").unwrap(), 0);
        assert_eq!(timezone_offset_secs("+08:00").unwrap(), 28800);
        assert_eq!(timezone_offset_secs("-05:00").unwrap(), -18000);
        assert!(timezone_offset_secs("America/New_York").is_err());
    }

    #[test]
    fn bad_cron_is_rejected() {
        assert!(parse_cron("0 24 * * *").is_err());
        assert!(parse_cron("a b c d e").is_err());
        assert!(parse_cron("* * *").is_err());
    }
}
