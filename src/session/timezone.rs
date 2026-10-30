//! Time zone detection and mapping from IANA names / local offsets to Windows Standard
//! Time Zone names, ensuring FreeRDP and Windows 365 Cloud PCs reflect the client's actual
//! time zone rather than defaulting to data center locations.

#[cfg(target_os = "windows")]
pub const UTC_WINDOWS_TIMEZONE: &str = "Coordinated Universal Time";

#[cfg(not(target_os = "windows"))]
pub const UTC_WINDOWS_TIMEZONE: &str = "UTC";

/// Maps an IANA time zone identifier (e.g. "Asia/Kolkata", "America/New_York") to the
/// matching Windows Standard Time Zone Name expected by FreeRDP's `/timezone:` argument.
#[must_use]
pub fn iana_to_windows_timezone(iana: &str) -> Option<&'static str> {
    let normalized = iana.trim();
    match normalized {
        // Americas - North
        "America/New_York"
        | "America/Detroit"
        | "America/Toronto"
        | "America/Montreal"
        | "America/Kentucky/Louisville"
        | "America/Indiana/Indianapolis" => Some("Eastern Standard Time"),
        "America/Chicago" | "America/Indiana/Knox" | "America/Winnipeg" | "America/Menominee" => {
            Some("Central Standard Time")
        }
        "America/Denver" | "America/Boise" | "America/Edmonton" | "America/Yellowknife" => {
            Some("Mountain Standard Time")
        }
        "America/Phoenix" => Some("US Mountain Standard Time"),
        "America/Los_Angeles" | "America/Vancouver" | "America/Tijuana" => {
            Some("Pacific Standard Time")
        }
        "America/Anchorage" | "America/Juneau" | "America/Sitka" | "America/Nome" => {
            Some("Alaskan Standard Time")
        }
        "Pacific/Honolulu" => Some("Hawaiian Standard Time"),
        "America/Halifax" | "America/Moncton" | "America/Glace_Bay" => {
            Some("Atlantic Standard Time")
        }
        "America/St_Johns" => Some("Newfoundland Standard Time"),

        // Americas - Central & South
        "America/Mexico_City" | "America/Monterrey" | "America/Merida" => {
            Some("Central Standard Time (Mexico)")
        }
        "America/Guatemala"
        | "America/Costa_Rica"
        | "America/El_Salvador"
        | "America/Managua"
        | "America/Tegucigalpa" => Some("Central America Standard Time"),
        "America/Bogota" | "America/Lima" | "America/Panama" | "America/Guayaquil" => {
            Some("SA Pacific Standard Time")
        }
        "America/Caracas" => Some("Venezuela Standard Time"),
        "America/Santiago" => Some("Pacific SA Standard Time"),
        "America/Sao_Paulo" => Some("E. South America Standard Time"),
        "America/Argentina/Buenos_Aires" | "America/Argentina/Cordoba" => {
            Some("Argentina Standard Time")
        }
        "America/Montevideo" => Some("Montevideo Standard Time"),

        // Europe
        "Europe/London" | "Europe/Dublin" | "Atlantic/Reykjavik" | "Europe/Lisbon" => {
            Some("GMT Standard Time")
        }
        "Europe/Paris" | "Europe/Brussels" | "Europe/Madrid" => Some("Romance Standard Time"),
        "Europe/Berlin" | "Europe/Amsterdam" | "Europe/Rome" | "Europe/Vienna"
        | "Europe/Stockholm" | "Europe/Oslo" | "Europe/Copenhagen" | "Europe/Zurich" => {
            Some("W. Europe Standard Time")
        }
        "Europe/Warsaw" | "Europe/Prague" | "Europe/Budapest" | "Europe/Belgrade"
        | "Europe/Bratislava" | "Europe/Ljubljana" | "Europe/Zagreb" | "Europe/Sarajevo" => {
            Some("Central European Standard Time")
        }
        "Europe/Helsinki" | "Europe/Athens" | "Europe/Bucharest" | "Europe/Sofia"
        | "Europe/Tallinn" | "Europe/Riga" | "Europe/Vilnius" | "Europe/Kyiv"
        | "Europe/Chisinau" => Some("FLE Standard Time"),
        "Europe/Moscow" | "Europe/Simferopol" | "Europe/Kirov" => Some("Russian Standard Time"),
        "Europe/Istanbul" | "Asia/Istanbul" => Some("Turkey Standard Time"),

        // Middle East & Africa
        "Asia/Jerusalem" | "Asia/Tel_Aviv" => Some("Israel Standard Time"),
        "Asia/Dubai" | "Asia/Muscat" => Some("Arabian Standard Time"),
        "Asia/Riyadh" | "Asia/Kuwait" | "Asia/Baghdad" | "Asia/Qatar" | "Asia/Bahrain" => {
            Some("Arabic Standard Time")
        }
        "Asia/Beirut" => Some("Middle East Standard Time"),
        "Africa/Cairo" => Some("Egypt Standard Time"),
        "Africa/Johannesburg" => Some("South Africa Standard Time"),
        "Africa/Casablanca" => Some("Morocco Standard Time"),
        "Africa/Nairobi" => Some("E. Africa Standard Time"),
        "Africa/Lagos" => Some("W. Central Africa Standard Time"),

        // Asia & Indian Subcontinent
        "Asia/Kolkata" | "Asia/Calcutta" | "Asia/Colombo" => Some("India Standard Time"),
        "Asia/Kathmandu" | "Asia/Katmandu" => Some("Nepal Standard Time"),
        "Asia/Dhaka" => Some("Bangladesh Standard Time"),
        "Asia/Karachi" => Some("Pakistan Standard Time"),
        "Asia/Tashkent" => Some("West Asia Standard Time"),
        "Asia/Almaty" => Some("Central Asia Standard Time"),
        "Asia/Bangkok" | "Asia/Jakarta" | "Asia/Ho_Chi_Minh" | "Asia/Phnom_Penh" => {
            Some("SE Asia Standard Time")
        }
        "Asia/Singapore" | "Asia/Hong_Kong" | "Asia/Taipei" | "Asia/Manila"
        | "Asia/Kuala_Lumpur" | "Asia/Macau" => Some("Singapore Standard Time"),
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Beijing" | "Asia/Urumqi" => {
            Some("China Standard Time")
        }
        "Asia/Tokyo" => Some("Tokyo Standard Time"),
        "Asia/Seoul" => Some("Korea Standard Time"),

        // Australia & Pacific
        "Australia/Perth" => Some("W. Australia Standard Time"),
        "Australia/Adelaide" | "Australia/Broken_Hill" => Some("Cen. Australia Standard Time"),
        "Australia/Darwin" => Some("AUS Central Standard Time"),
        "Australia/Sydney" | "Australia/Melbourne" | "Australia/Canberra" | "Australia/Hobart" => {
            Some("AUS Eastern Standard Time")
        }
        "Australia/Brisbane" => Some("E. Australia Standard Time"),
        "Pacific/Auckland" | "Pacific/Wellington" => Some("New Zealand Standard Time"),
        "Pacific/Fiji" => Some("Fiji Standard Time"),
        "Pacific/Guam" | "Pacific/Port_Moresby" => Some("West Pacific Standard Time"),

        // UTC
        "UTC" | "Etc/UTC" | "Etc/GMT" | "GMT" => Some(UTC_WINDOWS_TIMEZONE),
        _ => None,
    }
}

/// Fallback mapping from local UTC offset (in seconds) to a standard Windows time zone.
#[must_use]
pub fn offset_to_windows_timezone(offset_seconds: i32) -> Option<&'static str> {
    match offset_seconds {
        -36000 => Some("Hawaiian Standard Time"),       // -10:00
        -32400 => Some("Alaskan Standard Time"),        // -09:00
        -28800 => Some("Pacific Standard Time"),        // -08:00
        -25200 => Some("Mountain Standard Time"),       // -07:00
        -21600 => Some("Central Standard Time"),        // -06:00
        -18000 => Some("Eastern Standard Time"),        // -05:00
        -14400 => Some("Atlantic Standard Time"),       // -04:00
        -12600 => Some("Newfoundland Standard Time"),   // -03:30
        -10800 => Some("Argentina Standard Time"),      // -03:00
        -7200 => Some("UTC-02"),                        // -02:00
        -3600 => Some("Cape Verde Standard Time"),      // -01:00
        0 => Some(UTC_WINDOWS_TIMEZONE),                // +00:00
        3600 => Some("W. Europe Standard Time"),        // +01:00
        7200 => Some("FLE Standard Time"),              // +02:00
        10800 => Some("Russian Standard Time"),         // +03:00
        12600 => Some("Iran Standard Time"),            // +03:30
        14400 => Some("Arabian Standard Time"),         // +04:00
        16200 => Some("Afghanistan Standard Time"),     // +04:30
        18000 => Some("Pakistan Standard Time"),        // +05:00
        19800 => Some("India Standard Time"),           // +05:30
        20700 => Some("Nepal Standard Time"),           // +05:45
        21600 => Some("Bangladesh Standard Time"),      // +06:00
        23400 => Some("Myanmar Standard Time"),         // +06:30
        25200 => Some("SE Asia Standard Time"),         // +07:00
        28800 => Some("China Standard Time"),           // +08:00
        32400 => Some("Tokyo Standard Time"),           // +09:00
        34200 => Some("Cen. Australia Standard Time"),  // +09:30
        36000 => Some("AUS Eastern Standard Time"),     // +10:00
        37800 => Some("Lord Howe Standard Time"),       // +10:30
        39600 => Some("Central Pacific Standard Time"), // +11:00
        41400 => Some("Norfolk Standard Time"),         // +11:30
        43200 => Some("New Zealand Standard Time"),     // +12:00
        45900 => Some("Chatham Islands Standard Time"), // +12:45
        46800 => Some("Tonga Standard Time"),           // +13:00
        50400 => Some("Line Islands Standard Time"),    // +14:00
        _ => None,
    }
}

#[cfg(target_os = "windows")]
mod windows_tz {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SystemTime {
        w_year: u16,
        w_month: u16,
        w_day_of_week: u16,
        w_day: u16,
        w_hour: u16,
        w_minute: u16,
        w_second: u16,
        w_milliseconds: u16,
    }

    #[repr(C)]
    struct DynamicTimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime,
        daylight_bias: i32,
        time_zone_key_name: [u16; 128],
        dynamic_daylight_time_disabled: u8,
    }

    extern "system" {
        fn GetDynamicTimeZoneInformation(
            p_time_zone_information: *mut DynamicTimeZoneInformation,
        ) -> u32;
    }

    pub fn get_native_timezone() -> Option<String> {
        let mut info = std::mem::MaybeUninit::<DynamicTimeZoneInformation>::zeroed();
        let res = unsafe { GetDynamicTimeZoneInformation(info.as_mut_ptr()) };
        if res == 0xFFFF_FFFF {
            return None;
        }
        let info = unsafe { info.assume_init() };
        let key_len = info
            .time_zone_key_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.time_zone_key_name.len());
        if key_len > 0 {
            if let Ok(key) = String::from_utf16(&info.time_zone_key_name[..key_len]) {
                let trimmed = key.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        let std_len = info
            .standard_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.standard_name.len());
        if std_len > 0 {
            if let Ok(name) = String::from_utf16(&info.standard_name[..std_len]) {
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }
}

/// Detects the current system's Windows Standard Time Zone name.
#[must_use]
pub fn current_windows_timezone() -> Option<&'static str> {
    #[cfg(target_os = "windows")]
    {
        if let Some(native_name) = windows_tz::get_native_timezone() {
            if let Some(matched) = match_windows_timezone(&native_name) {
                return Some(matched);
            }
        }
    }

    if let Ok(iana) = iana_time_zone::get_timezone() {
        if let Some(win_tz) = iana_to_windows_timezone(&iana) {
            return Some(win_tz);
        }
    }
    // Fallback based on system local time offset
    let offset_secs = chrono::Local::now().offset().local_minus_utc();
    if let Some(win_tz) = offset_to_windows_timezone(offset_secs) {
        return Some(win_tz);
    }
    // Ultimate fallback if neither IANA nor offset could be matched
    Some(UTC_WINDOWS_TIMEZONE)
}

pub const SYNC_HOST_TIMEZONE: &str = "sync";

pub const COMMON_TIMEZONES: &[(&str, &str)] = &[
    ("sync", "Sync with Host OS (Default)"),
    (UTC_WINDOWS_TIMEZONE, "UTC (Coordinated Universal Time)"),
    ("Eastern Standard Time", "Eastern Time (US & Canada)"),
    ("Central Standard Time", "Central Time (US & Canada)"),
    ("Mountain Standard Time", "Mountain Time (US & Canada)"),
    ("Pacific Standard Time", "Pacific Time (US & Canada)"),
    ("GMT Standard Time", "GMT (London, Dublin, Lisbon)"),
    (
        "W. Europe Standard Time",
        "Western Europe (Berlin, Paris, Rome)",
    ),
    (
        "Central European Standard Time",
        "Central Europe (Warsaw, Prague)",
    ),
    (
        "FLE Standard Time",
        "Eastern Europe (Helsinki, Athens, Kyiv)",
    ),
    ("Israel Standard Time", "Israel (Jerusalem, Tel Aviv)"),
    ("Arabic Standard Time", "Arabia (Riyadh, Kuwait)"),
    ("Arabian Standard Time", "Gulf (Dubai, Muscat)"),
    ("India Standard Time", "India (New Delhi, Mumbai, Kolkata)"),
    ("SE Asia Standard Time", "SE Asia (Bangkok, Jakarta)"),
    (
        "China Standard Time",
        "China (Beijing, Shanghai, Hong Kong)",
    ),
    ("Tokyo Standard Time", "Japan (Tokyo, Osaka)"),
    ("AUS Eastern Standard Time", "Australia (Sydney, Melbourne)"),
    (
        "New Zealand Standard Time",
        "New Zealand (Auckland, Wellington)",
    ),
];

/// Matches a string case-insensitively against known Windows time zone names.
#[must_use]
pub fn match_windows_timezone(name: &str) -> Option<&'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.eq_ignore_ascii_case("UTC")
        || trimmed.eq_ignore_ascii_case("Coordinated Universal Time")
    {
        return Some(UTC_WINDOWS_TIMEZONE);
    }
    if let Some((id, _)) = COMMON_TIMEZONES
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(trimmed))
    {
        return Some(id);
    }
    ALL_WINDOWS_TIMEZONES
        .iter()
        .copied()
        .find(|&tz| tz.eq_ignore_ascii_case(trimmed))
}

/// Comprehensive list of standard Windows Time Zone names.
pub const ALL_WINDOWS_TIMEZONES: &[&str] = &[
    "Dateline Standard Time",
    "UTC-11",
    "Aleutian Standard Time",
    "Hawaiian Standard Time",
    "Marquesas Standard Time",
    "Alaskan Standard Time",
    "UTC-09",
    "Pacific Standard Time (Mexico)",
    "UTC-08",
    "Pacific Standard Time",
    "US Mountain Standard Time",
    "Mountain Standard Time (Mexico)",
    "Mountain Standard Time",
    "Yukon Standard Time",
    "Central America Standard Time",
    "Central Standard Time",
    "Easter Island Standard Time",
    "Central Standard Time (Mexico)",
    "Canada Central Standard Time",
    "SA Pacific Standard Time",
    "Eastern Standard Time (Mexico)",
    "Eastern Standard Time",
    "Haiti Standard Time",
    "Cuba Standard Time",
    "US Eastern Standard Time",
    "Turks And Caicos Standard Time",
    "Paraguay Standard Time",
    "Atlantic Standard Time",
    "Venezuela Standard Time",
    "Central Brazilian Standard Time",
    "SA Western Standard Time",
    "Pacific SA Standard Time",
    "Newfoundland Standard Time",
    "Tocantins Standard Time",
    "E. South America Standard Time",
    "SA Eastern Standard Time",
    "Argentina Standard Time",
    "Greenland Standard Time",
    "Montevideo Standard Time",
    "Magallanes Standard Time",
    "Saint Pierre Standard Time",
    "Bahia Standard Time",
    "UTC-02",
    "Mid-Atlantic Standard Time",
    "Azores Standard Time",
    "Cape Verde Standard Time",
    "UTC",
    "Coordinated Universal Time",
    "GMT Standard Time",
    "Greenwich Standard Time",
    "Sao Tome Standard Time",
    "Morocco Standard Time",
    "W. Europe Standard Time",
    "Central Europe Standard Time",
    "Romance Standard Time",
    "Central European Standard Time",
    "W. Central Africa Standard Time",
    "Jordan Standard Time",
    "GTB Standard Time",
    "Middle East Standard Time",
    "Egypt Standard Time",
    "E. Europe Standard Time",
    "Syria Standard Time",
    "West Bank Standard Time",
    "South Africa Standard Time",
    "FLE Standard Time",
    "Israel Standard Time",
    "South Sudan Standard Time",
    "Kaliningrad Standard Time",
    "Sudan Standard Time",
    "Libya Standard Time",
    "Namibia Standard Time",
    "Arabic Standard Time",
    "Turkey Standard Time",
    "Arab Standard Time",
    "Belarus Standard Time",
    "Russian Standard Time",
    "E. Africa Standard Time",
    "Iran Standard Time",
    "Arabian Standard Time",
    "Astrakhan Standard Time",
    "Azerbaijan Standard Time",
    "Russia Time Zone 3",
    "Mauritius Standard Time",
    "Saratov Standard Time",
    "Georgian Standard Time",
    "Volgograd Standard Time",
    "Caucasus Standard Time",
    "Afghanistan Standard Time",
    "West Asia Standard Time",
    "Ekaterinburg Standard Time",
    "Pakistan Standard Time",
    "Qyzylorda Standard Time",
    "India Standard Time",
    "Sri Lanka Standard Time",
    "Nepal Standard Time",
    "Central Asia Standard Time",
    "Bangladesh Standard Time",
    "Omsk Standard Time",
    "Myanmar Standard Time",
    "SE Asia Standard Time",
    "Altai Standard Time",
    "W. Mongolia Standard Time",
    "North Asia Standard Time",
    "N. Central Asia Standard Time",
    "Tomsk Standard Time",
    "China Standard Time",
    "North Asia East Standard Time",
    "Singapore Standard Time",
    "W. Australia Standard Time",
    "Taipei Standard Time",
    "Ulaanbaatar Standard Time",
    "Aus Central W. Standard Time",
    "Transbaikal Standard Time",
    "Tokyo Standard Time",
    "North Korea Standard Time",
    "Korea Standard Time",
    "Yakutsk Standard Time",
    "Cen. Australia Standard Time",
    "AUS Central Standard Time",
    "E. Australia Standard Time",
    "AUS Eastern Standard Time",
    "West Pacific Standard Time",
    "Tasmania Standard Time",
    "Vladivostok Standard Time",
    "Lord Howe Standard Time",
    "Bougainville Standard Time",
    "Russia Time Zone 10",
    "Magadan Standard Time",
    "Norfolk Standard Time",
    "Sakhalin Standard Time",
    "Central Pacific Standard Time",
    "Russia Time Zone 11",
    "New Zealand Standard Time",
    "UTC+12",
    "Fiji Standard Time",
    "Chatham Islands Standard Time",
    "UTC+13",
    "Tonga Standard Time",
    "Samoa Standard Time",
    "Line Islands Standard Time",
];

/// Resolves the Windows timezone to pass to FreeRDP based on user preference.
#[must_use]
pub fn resolve_windows_timezone(preference: &str) -> Option<&'static str> {
    let trimmed = preference.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case(SYNC_HOST_TIMEZONE)
        || trimmed.eq_ignore_ascii_case("auto")
    {
        current_windows_timezone()
    } else if let Some(tz) = iana_to_windows_timezone(trimmed) {
        Some(tz)
    } else {
        match_windows_timezone(trimmed)
    }
}

/// Returns a human-readable label for a timezone preference id.
#[must_use]
pub fn timezone_label(preference: &str) -> &'static str {
    let trimmed = preference.trim();
    for (id, label) in COMMON_TIMEZONES {
        if id.eq_ignore_ascii_case(trimmed) {
            return label;
        }
    }
    "Sync with Host OS (Default)"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_iana_timezones() {
        assert_eq!(
            iana_to_windows_timezone("Asia/Kolkata"),
            Some("India Standard Time")
        );
        assert_eq!(
            iana_to_windows_timezone("America/New_York"),
            Some("Eastern Standard Time")
        );
        assert_eq!(
            iana_to_windows_timezone("Europe/London"),
            Some("GMT Standard Time")
        );
        assert_eq!(
            iana_to_windows_timezone("Europe/Berlin"),
            Some("W. Europe Standard Time")
        );
        assert_eq!(
            iana_to_windows_timezone("Asia/Tokyo"),
            Some("Tokyo Standard Time")
        );
        assert_eq!(
            iana_to_windows_timezone("Australia/Sydney"),
            Some("AUS Eastern Standard Time")
        );
        assert_eq!(iana_to_windows_timezone("UTC"), Some(UTC_WINDOWS_TIMEZONE));
    }

    #[test]
    fn maps_offset_fallback() {
        assert_eq!(
            offset_to_windows_timezone(19800),
            Some("India Standard Time")
        );
        assert_eq!(
            offset_to_windows_timezone(-18000),
            Some("Eastern Standard Time")
        );
        assert_eq!(offset_to_windows_timezone(0), Some(UTC_WINDOWS_TIMEZONE));
    }

    #[test]
    fn detects_current_timezone() {
        let tz = current_windows_timezone();
        assert!(
            tz.is_some(),
            "Expected current timezone to resolve to a Windows standard name"
        );
    }

    #[test]
    fn resolves_timezone_preferences() {
        assert_eq!(
            resolve_windows_timezone("Eastern Standard Time"),
            Some("Eastern Standard Time")
        );
        assert_eq!(
            resolve_windows_timezone("America/New_York"),
            Some("Eastern Standard Time")
        );
        assert!(resolve_windows_timezone("sync").is_some());
    }

    #[test]
    fn matches_known_windows_timezones() {
        assert_eq!(
            match_windows_timezone("Pacific Standard Time"),
            Some("Pacific Standard Time")
        );
        assert_eq!(
            match_windows_timezone("India Standard Time"),
            Some("India Standard Time")
        );
        assert_eq!(match_windows_timezone("UTC"), Some(UTC_WINDOWS_TIMEZONE));
        assert_eq!(
            match_windows_timezone("Coordinated Universal Time"),
            Some(UTC_WINDOWS_TIMEZONE)
        );
        assert_eq!(match_windows_timezone("Nonexistent TZ"), None);
    }
}
