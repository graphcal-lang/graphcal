use graphcal_compiler::registry::time_zone::TimeZoneRegistry;

/// Parse a civil datetime string in a given IANA timezone and return a UTC `hifitime::Epoch`.
///
/// Uses Graphcal's explicit bundled timezone registry to resolve the civil time,
/// then converts the resulting instant to hifitime.
pub(super) fn datetime_with_timezone(
    datetime_str: &str,
    tz_name: &str,
    time_zones: &TimeZoneRegistry,
) -> Result<hifitime::Epoch, Box<dyn std::error::Error>> {
    let civil_dt: jiff::civil::DateTime = datetime_str.parse()?;
    let tz = time_zones.get(tz_name)?;
    let zdt = tz.to_zoned(civil_dt)?;
    let ts = zdt.timestamp();
    #[expect(
        clippy::arithmetic_side_effects,
        clippy::cast_precision_loss,
        reason = "hifitime exposes Epoch + Duration for this validated timestamp conversion"
    )]
    let epoch = hifitime::Epoch::from_unix_seconds(ts.as_second() as f64)
        + hifitime::Duration::from_nanoseconds(f64::from(ts.subsec_nanosecond()));
    Ok(epoch)
}
