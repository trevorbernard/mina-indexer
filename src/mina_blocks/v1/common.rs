pub(crate) fn from_str<'de, T, D>(de: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    use serde::Deserialize;
    Ok(String::deserialize(de)?
        .parse()
        .map_err(serde::de::Error::custom)?)
}
