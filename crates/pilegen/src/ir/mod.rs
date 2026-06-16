#![allow(dead_code)]
//! MTG Intermediate Representation (IR).
//!
//! A data-based DSL for expressing MTG card behavior as pure data, interpreted
//! by a generic executor. See `/home/bo/.claude/plans/i-d-like-to-discuss-squishy-bachman.md`
//! for the full design rationale.
//!
//! Sub-languages:
//! - `expr` — pure queries over game state (no side effects)
//! - `action` — state mutations, dispatched by the executor
//! - `ability` — wrappers (triggered / replacement / prohibition / static / activated)
//! - `ce` — continuous-effect modifications, composition primitive
//! - `context` — pointers into current event / cast / triggering frame
//! - `event_log` — append-only record of game events (Layer B)
//! - `executor` — interprets actions, evaluates expressions, matches filters

pub(crate) mod expr;
pub(crate) mod action;
pub(crate) mod ability;
pub(crate) mod ce;
pub(crate) mod context;
pub(crate) mod event_log;
pub(crate) mod executor;
pub(crate) mod cost;
pub(crate) mod cost_exec;

#[cfg(test)]
mod tests;
