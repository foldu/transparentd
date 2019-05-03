use cfgen::prelude::*;
use serde::de::{Deserialize, Deserializer, Visitor};
use serde_derive::Deserialize;

#[derive(Debug, Copy, Clone)]
pub struct Opacity(f64);

impl Opacity {
    pub fn new(opacity: f64) -> Option<Self> {
        if opacity >= 0.0 && opacity <= 1.0 {
            Some(Self(opacity))
        } else {
            None
        }
    }

    pub fn max() -> Self {
        Self(1.0)
    }
}

impl std::fmt::Display for Opacity {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(fmt, "{:3}", self.0)
    }
}

struct OpacityVisitor;

impl<'de> Visitor<'de> for OpacityVisitor {
    type Value = Opacity;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an opacity value between 0.0 and 1.0")
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Opacity::new(value).ok_or_else(|| {
            E::custom(format!(
                "float out of range: {}, must be between 0.0 and 1.0",
                value
            ))
        })
    }
}

impl<'de> Deserialize<'de> for Opacity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_f64(OpacityVisitor)
    }
}

const DEFAULT: &str = "\
transparency_at_start = true
opacity = 0.8
";

#[derive(Cfgen, Deserialize, Debug)]
#[cfgen(default = "DEFAULT")]
pub struct Config {
    pub transparency_at_start: bool,
    pub opacity: Opacity,
}
