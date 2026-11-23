use crate::filter::traits::FilterPolicy;

pub mod builder;
pub mod reader;

pub const FILTER_BASE_LG: u8 = 11;

pub fn filter_meta_key(policy: &dyn FilterPolicy) -> String {
    format!("filter.{}", policy.name())
}
