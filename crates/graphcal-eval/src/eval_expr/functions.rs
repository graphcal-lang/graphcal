/// Parse a civil datetime string in a given IANA timezone and return a UTC `hifitime::Epoch`.
///
/// Uses jiff to resolve the civil time to a UTC instant, then converts to hifitime.
pub(super) fn datetime_with_timezone(
    datetime_str: &str,
    tz_name: &str,
) -> Result<hifitime::Epoch, Box<dyn std::error::Error>> {
    let civil_dt: jiff::civil::DateTime = datetime_str.parse()?;
    let tz = jiff::tz::TimeZone::get(tz_name)?;
    let zdt = tz.to_zoned(civil_dt)?;
    let ts = zdt.timestamp();
    #[expect(
        clippy::cast_precision_loss,
        reason = "unix seconds for reasonable dates fit within f64 mantissa precision"
    )]
    let epoch = hifitime::Epoch::from_unix_seconds(ts.as_second() as f64)
        + hifitime::Duration::from_nanoseconds(f64::from(ts.subsec_nanosecond()));
    Ok(epoch)
}
