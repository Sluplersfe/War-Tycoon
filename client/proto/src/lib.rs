//! Generated wire types shared with the Go server (proto/frontline.proto).
#![allow(clippy::all)]

pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/frontline.v1.rs"));
}

pub use pb::*;
pub use prost::Message;
