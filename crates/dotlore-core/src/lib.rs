//! Core engine for dotlore: staging git repos and per-device cloud bundles.

pub mod git;
pub mod mirror;
pub mod cloud;
pub mod config;
pub mod repo;
pub mod conflict;
pub mod engine;
pub mod daemon;
