//! The one combine every stack goes through: [`StackConfig::combine`] reduces the cached frames
//! to the stacked product, under the settings [`StackConfig::combine_cached`] validated.
//!
//! [`StackConfig::combine`]: crate::combine::stack_config::StackConfig::combine
//! [`StackConfig::combine_cached`]: crate::combine::stack_config::StackConfig::combine_cached

pub(crate) mod combine_cost;
pub(crate) mod planned_combine;
pub(crate) mod quantization;
pub(crate) mod rejected_mean;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
