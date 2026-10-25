use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use uuid::Uuid;

/// Full-width counters cross JSON as canonical decimal strings, never JS numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Counter(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CounterExhausted;
impl std::fmt::Display for CounterExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("counter exhausted")
    }
}
impl std::error::Error for CounterExhausted {}
impl Counter {
    pub const ZERO: Self = Self(0);
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
    pub fn checked_next(self) -> Result<Self, CounterExhausted> {
        self.0.checked_add(1).map(Self).ok_or(CounterExhausted)
    }
}
impl Serialize for Counter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Counter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || !value.bytes().all(|b| b.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(D::Error::custom(
                "expected canonical unsigned decimal counter",
            ));
        }
        value.parse::<u64>().map(Self).map_err(D::Error::custom)
    }
}
macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "Uuid", into = "Uuid")]
        pub struct $name(Uuid);
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl TryFrom<Uuid> for $name {
            type Error = &'static str;
            fn try_from(value: Uuid) -> Result<Self, Self::Error> {
                if value.is_nil() {
                    Err("nil identity")
                } else {
                    Ok(Self(value))
                }
            }
        }
        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}
identity!(SessionId);
identity!(OccurrenceId);
identity!(VisitId);
identity!(SourceId);
identity!(BackendInstanceId);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_width_counter_roundtrip_and_number_rejection() {
        let value = Counter::new(u64::MAX);
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, "\"18446744073709551615\"");
        assert_eq!(serde_json::from_str::<Counter>(&json).unwrap(), value);
        for invalid in [
            "1",
            "9007199254740993",
            "\"01\"",
            "\"+1\"",
            "\"18446744073709551616\"",
        ] {
            assert!(serde_json::from_str::<Counter>(invalid).is_err());
        }
    }
    #[test]
    fn counters_never_wrap_and_nil_identity_is_not_valid_state() {
        assert_eq!(Counter::new(u64::MAX).checked_next(), Err(CounterExhausted));
        assert!(
            serde_json::from_str::<OccurrenceId>("\"00000000-0000-0000-0000-000000000000\"")
                .is_err()
        );
    }
}
