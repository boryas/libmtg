//! Pure-query sub-language. Expressions never mutate state.
//!
//! A `Filter` is just an `Expr` that evaluates to `Bool` — no dedicated enum.
//! Sugar helpers (e.g. `type_is(Creature)`) compose `Expr` trees in source.

use crate::ir::context::{Ctx, GameCtx};
use crate::{CardType, Color, CounterType, Keyword, Supertype, ZoneId};

/// Pure queries over game state. No side effects.
#[derive(Debug, Clone)]
pub enum Expr {
    // ── literals / context ───────────────────────────────────────────────
    Num(i64),
    Bool(bool),
    /// Reference to the ability's source object / its controller / the triggering
    /// event / a user-bound variable. See `Ctx` for full list.
    Ctx(Ctx),
    /// Layer C designation (monarch, day/night, etc.).
    GameCtx(GameCtx),

    // ── property projections (object → value) ────────────────────────────
    Types(Box<Expr>),            // Vec<CardType>
    Supertypes(Box<Expr>),       // Vec<Supertype>
    Subtypes(Box<Expr>),         // Vec<String> (creature/land/artifact subtypes)
    Colors(Box<Expr>),           // Vec<Color>
    Keywords(Box<Expr>),         // Vec<Keyword>
    Power(Box<Expr>),            // i64
    Toughness(Box<Expr>),        // i64
    Mv(Box<Expr>),               // i64 (mana value)
    Controller(Box<Expr>),       // PlayerId
    Owner(Box<Expr>),            // PlayerId
    ZoneOf(Box<Expr>),           // ZoneId
    ZoneLit(crate::ZoneId),      // a zone literal, for `ZoneOf(It) == ZoneLit(z)`
    ObjLit(crate::ObjId),        // an object-id literal, for `It == ObjLit(id)` exclusion
    IsToken(Box<Expr>),          // Bool — true iff `obj` is a token
    IsAbility(Box<Expr>),        // Bool — true iff `obj` is a card-less ability on the stack
    AbilityIsTriggered(Box<Expr>), // Bool — true iff `obj` is a *triggered* ability (vs activated)
    CountersOn(Box<Expr>, CounterType), // i64
    Name(Box<Expr>),             // String

    // ── battlefield-state projections ────────────────────────────────────
    /// True iff `obj` is on the battlefield with `attacking = true`.
    Attacking(Box<Expr>),        // Bool
    /// True iff `obj` is on the battlefield with `unblocked = true` (an
    /// attacker that wasn't blocked this combat). Used by ninjutsu's
    /// "return an unblocked attacker" cost filter.
    Unblocked(Box<Expr>),        // Bool

    // ── player projections ───────────────────────────────────────────────
    Life(Box<Expr>),             // i64
    HandSize(Box<Expr>),         // i64
    Opponents(Box<Expr>),        // Vec<PlayerId>

    // ── zone projections ─────────────────────────────────────────────────
    /// Top N cards of a zone (typically library).
    Top { zone: ZoneSel, n: Box<Expr> },

    // ── boolean / arithmetic ─────────────────────────────────────────────
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    /// Set membership: does lhs (scalar) appear in rhs (set)?
    Contains(Box<Expr>, Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Max(Box<Expr>, Box<Expr>),

    // ── set-builder and folds ────────────────────────────────────────────
    /// The players, as objects (`ObjSet` of the player `GameObject`s). Players are
    /// first-class objects, so "each player" is a `ForEach` over this set.
    Players,
    /// All objects in a zone matching a filter, bound to a variable name in
    /// the filter body. Result is a set of object refs.
    AllObjects {
        zone: ZoneSel,
        bind: &'static str,
        filter: Box<Expr>, // evaluated with `bind` → candidate object
    },
    /// |set|
    Count(Box<Expr>),
    /// ∃ x ∈ set. body(x) — `bind` names the element in `body`.
    Any { set: Box<Expr>, bind: &'static str, body: Box<Expr> },
    /// ∀ x ∈ set. body(x)
    All { set: Box<Expr>, bind: &'static str, body: Box<Expr> },

    // ── literal collections ──────────────────────────────────────────────
    TypeLit(CardType),
    SupertypeLit(Supertype),
    SubtypeLit(String),
    ColorLit(Color),
    KeywordLit(Keyword),
    NameLit(String),

    // ── binding ──────────────────────────────────────────────────────────
    Let { name: &'static str, value: Box<Expr>, body: Box<Expr> },

    // ── runtime environment inspection ───────────────────────────────────
    /// True iff `name` is currently bound in the env to a non-Unit value.
    /// Use to gate on optional targeting ("up to one ~") without a sentinel.
    Bound(&'static str),

    // ── event log (Layer B) ──────────────────────────────────────────────
    /// |{ e in event_log[window] : filter(e) }|.
    /// Subsumes the scattered `this_turn`-shaped counters (spells_cast_this_turn,
    /// draws_this_turn, etc.). Semantics are defined by the filter; see
    /// `EventFilter` for the closed vocabulary.
    EventCount {
        window: crate::ir::event_log::Window,
        filter: Box<EventFilter>,
    },
}

/// Predicate over logged events for Layer B folds. Kept minimal and grows
/// demand-driven — each variant maps to one `GameEvent` family with the
/// specific field selectors that show up in real cards.
#[derive(Debug, Clone)]
pub enum EventFilter {
    /// A spell was cast. `caster` optionally filters by the player who cast it
    /// (resolves against the event's `caster` field). `spell_filter` optionally
    /// filters by the cast object itself (its `card_id`, resolved against the
    /// catalog) — e.g. "noncreature spells this turn" for The Fantasticar.
    SpellCast {
        caster: Option<Box<Expr>>,
        spell_filter: Option<Box<Filter>>,
    },
}

/// Which zone to scan, possibly controller-scoped.
#[derive(Debug, Clone)]
pub enum ZoneSel {
    /// Absolute zone reference (e.g. a specific object's current zone kind).
    Id(ZoneId),
    /// "Your graveyard", "any opponent's library", etc. — resolved against
    /// the current binding environment.
    Scoped { zone_kind: ZoneKindSel, owner: Box<Expr> },
    /// All zones of a given kind across all players (e.g. "the battlefield").
    Global(ZoneKindSel),
}

/// Zone kinds as a selector — distinct from `ZoneId` which is the engine's
/// already-instantiated-per-player enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneKindSel {
    Library,
    Hand,
    Battlefield,
    Graveyard,
    Exile,
    Stack,
    Command,
}

/// Evaluation result. Union over the kinds of values Expr can produce.
#[derive(Debug, Clone)]
pub enum Value {
    Num(i64),
    Bool(bool),
    Obj(crate::ObjId),
    Player(crate::PlayerId),
    Zone(ZoneId),
    Type(CardType),
    Supertype(Supertype),
    Subtype(String),
    Color(Color),
    Keyword(Keyword),
    Counter(CounterType),
    Name(String),
    ObjSet(Vec<crate::ObjId>),
    PlayerSet(Vec<crate::PlayerId>),
    TypeSet(Vec<CardType>),
    SupertypeSet(Vec<Supertype>),
    ColorSet(Vec<Color>),
    KeywordSet(Vec<Keyword>),
    SubtypeSet(Vec<String>),
    Unit,
}

/// Readability newtype — signatures that want "a predicate" can say so.
#[derive(Debug, Clone)]
pub struct Filter(pub Expr);
