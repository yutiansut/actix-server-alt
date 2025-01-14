//! Http module for [Service](actix_service_alt::Service) trait oriented http handling.
//!
//! This crate tries to serve both low overhead and ease of use purpose.
//! All http protocols can be used separately with corresponding feature flag or work together
//! for handling different protocols in one place.

#![forbid(unsafe_code)]
#![allow(incomplete_features)]
#![feature(generic_associated_types, min_type_alias_impl_trait)]

mod body;
mod builder;
mod error;
mod expect;
mod flow;
mod protocol;
mod response;
mod service;
mod tls;
mod upgrade;

#[cfg(feature = "http1")]
pub mod h1;
#[cfg(feature = "http2")]
pub mod h2;
#[cfg(feature = "http3")]
pub mod h3;

pub mod config;
pub mod util;

/// re-export http crate as module.
pub use http;

pub use body::{RequestBody, ResponseBody};
pub use builder::HttpServiceBuilder;
pub use error::{BodyError, HttpServiceError};
pub use response::ResponseError;
pub use service::HttpService;

// temporary compile error for conflicted feature combination.
#[cfg(not(feature = "http1"))]
#[cfg(all(feature = "http2", feature = "native-tls"))]
compile_error!("http2 feature can not use native-tls");
