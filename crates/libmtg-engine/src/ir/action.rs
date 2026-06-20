//! Mutation sub-language. `Action` variants describe *structural* operations;
//! named MTG mechanics (flashback, cascade, pump-until-EOT) are compositions
//! built via sugar helpers, not Action variants.

use crate::ir::ce::CEMod;
use crate::ir::expr::{Expr, Filter, ZoneKindSel};
use crate::CounterType;

/// How long a `CEMod` application lasts.
#[derive(Clone)]
pub enum Expiry {
    EndOfTurn,
    EndOfCombat,
    UntilYourNextTurn,
    WhileSourcePresent,
    Permanent,
}

/// Verb tag on `Action::MoveByChoice` — disambiguates the event family that
/// fires when the chosen objects shift zones. The (from, to) pair alone
/// isn't enough: bf→gy can be Sacrifice (CR 701.16 triggers) or a Destroy
/// effect's zone movement; hand→gy is Discard (CR 701.8 triggers); etc.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MoveVerb {
    /// "Return ~ to its owner's hand" / "Bounce ~ to library" — the standard
    /// zone-change family. No special trigger family beyond zone-change.
    Return,
    /// "Exile ~" — fires exile triggers (CR 701.18) in addition to zone-change.
    Exile,
    /// "Sacrifice ~" — fires sacrifice triggers (CR 701.16) in addition to
    /// zone-change. Tokens cease to exist on graveyard arrival (CR 704.5d).
    Sacrifice,
    /// "Discard ~" — fires discard triggers (CR 701.8) in addition to
    /// zone-change. Madness etc. trigger off this event family.
    Discard,
}

/// Who is performing the action / receiving the choice.
#[derive(Clone)]
pub enum Who {
    You,
    EachOpponent,
    Opponent,
    Player(Expr), // resolves to PlayerId
    Each,         // all players in APNAP order
}

/// Selector for a choose-one / choose-mode effect.
///
/// `cost` (if present) must be paid by the chooser to pick this option; the
/// executor filters out unpayable options before presenting the remaining set
/// to the strategy. This is the structural decomposition of "unless X pays Y"
/// patterns — no `CounterUnlessPays`-style named primitive exists by design.
#[derive(Clone)]
pub struct ChoiceOption {
    pub label: &'static str,
    /// Cost to pick this option (if present); the executor filters out
    /// unpayable options before offering the chooser. `None` = free option.
    pub cost: Option<Box<Action>>,
    pub action: Box<Action>,
}

/// One-shot mutations.
#[derive(Clone)]
pub enum Action {
    // ── state movement ───────────────────────────────────────────────────
    /// Move `what` to zone `to`. The object's current zone is read from state
    /// — no `from` field is required. (`change_zone` handles all departures
    /// uniformly.)
    Move {
        what: Expr,             // object or set
        to: ZoneKindSel,
        to_owner: Option<Expr>, // default: same owner
        bind_as: Option<&'static str>,
    },
    Search {
        who: Who,
        zone: ZoneKindSel,
        filter: Filter,
        count: Expr,
        dest: ZoneKindSel,
        /// When `dest` is `Library`, place the found card(s) on TOP rather than
        /// the (default) bottom — for tutors that put on top (Personal Tutor,
        /// Vampiric/Mystical Tutor). Ignored for other destinations.
        to_top: bool,
        shuffle: bool,
        bind_as: Option<&'static str>,
    },
    Return {
        what: Expr,
        to: ZoneKindSel,
        bind_as: Option<&'static str>,
    },
    Discard {
        who: Who,
        count: Expr,
        at_random: bool,
        filter: Option<Filter>,
    },
    Mill {
        who: Who,
        count: Expr,
    },
    /// Shuffle `who`'s library into a random order. CR 701.20. The randomisation
    /// itself carries no player agency, so there is no decision here — "you *may*
    /// shuffle" is `MayDo { Shuffle }`, and "search, then shuffle" composes
    /// `Search { shuffle: false }` with this (the `Search.shuffle` flag is a
    /// convenience for the common fetch case).
    Shuffle {
        who: Who,
    },

    // ── stack / casting ──────────────────────────────────────────────────
    Counter {
        target: Expr,
    },
    /// "Offer to cast X" — subsumes cast-without-paying, flashback, cascade,
    /// madness, Snapcaster, etc. All are `OfferCast` with different
    /// `permissions` CEMods.
    OfferCast {
        what: Expr,
        permissions: Vec<CEMod>,
    },
    /// CR 706: create `n` copies of the spell referenced by `what` as stack
    /// objects. Each copy resolves with the same effect as the original.
    /// Subsumes storm (n = spells-cast-this-turn), Reverberate (n = 1),
    /// Thousand-Year Storm, fork, etc.
    ///
    /// `new_targets`: if true, the controller of each copy may pick new
    /// targets (CR 706.10f); otherwise the copy inherits the original's
    /// targets. Engine default: prefer legal targets not yet hit.
    CopySpell {
        what: Expr,
        n: Expr,
        new_targets: bool,
    },

    // ── player effects ───────────────────────────────────────────────────
    Draw {
        who: Who,
        n: Expr,
    },
    DealDamage {
        source: Expr,
        target: Expr,
        amount: Expr,
    },
    PayLife {
        who: Who,
        amount: Expr,
    },
    GainLife {
        who: Who,
        amount: Expr,
    },

    // ── counters ─────────────────────────────────────────────────────────
    PutCounters {
        on: Expr,
        kind: CounterType,
        n: Expr,
    },
    RemoveCounters {
        from: Expr,
        kind: CounterType,
        n: Expr,
    },

    // ── tap / untap ──────────────────────────────────────────────────────
    /// Tap a permanent. CR 701.20a. Universal primitive — used by direct
    /// effects ("tap target permanent"), replacement bodies that compose
    /// "enters tapped" as `Sequence([Move, Tap])`, and cost payment
    /// (e.g. a tap-self mana ability).
    Tap {
        target: Expr,
    },
    /// Untap a permanent. CR 701.21a. Symmetric with `Tap`.
    Untap {
        target: Expr,
    },
    /// Transform a double-faced permanent *in place* — flip to its other face
    /// (CR 712.4 / 701.28). Same object; if the new face is a planeswalker it
    /// gains its printed starting loyalty (CR 711.3c). Fires `Transformed`.
    /// Used for literal "transform ~" cards (Delver). "Exile, then return
    /// transformed" (Tamiyo) is *not* this — it's a new object, modeled as
    /// `Sequence([Exile, Move→bf, Transform])`.
    Transform {
        target: Expr,
    },
    /// Attach `what` (an Equipment or Aura) to permanent `to` (CR 701.3 /
    /// 702.6). Sets `what.attached_to = to` and fires `BecameAttached` so
    /// "whenever ~ becomes equipped/enchanted" triggers can react. Generic
    /// across the equip ability, Living Weapon's auto-attach, and Aura ETB.
    /// Detachment is not a separate action — `change_zone` clears `attached_to`
    /// when either object leaves the battlefield (CR 704.5q).
    Attach {
        what: Expr,
        to: Expr,
    },

    // ── destruction / targeting ──────────────────────────────────────────
    Destroy {
        target: Expr,
    },
    Exile {
        target: Expr,
        bind_as: Option<&'static str>,
    },
    Sacrifice {
        who: Who,
        filter: Filter,
        count: Expr,
        bind_as: Option<&'static str>,
    },
    /// Player picks `count` objects matching `filter` from `from` zone and
    /// moves them to `to` zone. Subsumes return-to-hand, exile-from-hand,
    /// return-from-graveyard, exile-from-graveyard, etc. — anywhere a
    /// player chooses K from a filtered pool and the chosen objects shift
    /// zones.
    ///
    /// `verb` disambiguates event semantics: the same (from, to) shape can
    /// fire different event families (e.g. bf→gy is Sacrifice (CR 701.16
    /// triggers) vs. Destroy-effect zone movement). Carrying the verb
    /// explicitly avoids inferring intent from zone shape.
    ///
    /// `bind_as: Some(name)` is required for cost-tree usage — the schema
    /// decision is keyed under `name` so the executor's BindEnv readback
    /// finds the strategy's choice. Existing `Sacrifice`/`Discard` use a
    /// callback for selection; once those switch to binding-driven
    /// execution they collapse into this variant.
    MoveByChoice {
        who: Who,
        from: ZoneKindSel,
        to: ZoneKindSel,
        verb: MoveVerb,
        filter: Filter,
        count: Expr,
        bind_as: Option<&'static str>,
    },

    // ── continuous-effect application ────────────────────────────────────
    /// Apply a bundle of CE modifications to `target` until `expiry`.
    /// Subsumes pump-until-EOT, grant-flash, gain-protection, etc.
    ApplyCE {
        target: Expr,
        mods: Vec<CEMod>,
        expiry: Expiry,
    },

    // ── control flow ─────────────────────────────────────────────────────
    Sequence(Vec<Action>),
    IfThen {
        cond: Expr,
        then: Box<Action>,
        else_: Option<Box<Action>>,
    },
    MayDo {
        who: Who,
        action: Box<Action>,
    },
    ForEach {
        over: Expr,           // set
        bind: &'static str,
        body: Box<Action>,
    },
    Choose {
        who: Who,
        prompt: &'static str,
        options: Vec<ChoiceOption>,
        /// Cost-context binding (CR 601.2b). When `Some(name)` and the
        /// `BindEnv` holds a `Branch` answer under `name`, the executor takes
        /// the pre-decided option and runs its action against the same env (so
        /// the option's nested cost decisions resolve). When `None` — the
        /// effect-resolution case — the chooser is asked via `resolve_choice`.
        bind_as: Option<&'static str>,
    },

    // ── scheduling ───────────────────────────────────────────────────────
    /// Register a delayed trigger that fires at some future event.
    ScheduleDelayedTrigger {
        fires: crate::ir::ability::TriggerSpec,
        action: Box<Action>,
    },
    /// CR 611.2f: register a latent continuous effect that applies to the
    /// next qualifying spell `who` casts. The `mods` bundle is applied as a
    /// continuous instance filtered to that spell; the LatentSpellMod itself
    /// is consumed once a matching spell is announced. `expiry` governs both
    /// the latent registration (if no qualifying spell is cast in time) and
    /// the applied CE.
    GrantCEToNextSpellCast {
        who: Who,
        predicate: Option<Filter>,
        mods: Vec<CEMod>,
        expiry: Expiry,
    },

    // ── information ──────────────────────────────────────────────────────
    Scry {
        who: Who,
        n: Expr,
    },
    Surveil {
        who: Who,
        n: Expr,
    },
    /// Look at the top `n` cards of `who`'s library and put them back in a
    /// player-chosen order (Ponder, "put back in any order"). The arrangement is
    /// a decision, routed through `Strategy::order_top_library` — not an engine sort.
    OrderTop {
        who: Who,
        n: Expr,
    },
    /// Look at the top `n` cards of `who`'s library; the player puts `take` of
    /// them into hand and the rest on the bottom (any order). The kept cards are
    /// a plain move into hand — NOT a draw (no Draw event/triggers fire). The
    /// to-hand choice routes through `Strategy::choose_for_effect`. Sibling of
    /// Scry/Surveil/OrderTop in the look-at-top family (Flow State, Stock Up).
    Dig {
        who: Who,
        n: Expr,
        take: Expr,
    },
    Reveal {
        who: Who,
        what: Expr,
    },
    Look {
        who: Who,
        zone: ZoneKindSel,
        n: Expr,
    },

    // ── token creation ───────────────────────────────────────────────────
    CreateToken {
        who: Who,
        spec: TokenSpec,
        n: Expr,
    },

    // ── mana production ──────────────────────────────────────────────────
    /// Add `count` mana to `who`'s mana pool. The `spec` describes the colors
    /// produced; for `AnyOneColor`, the chosen color is read from
    /// `BindEnv.chosen_color` (set by the activated-ability dispatch when the
    /// player picks a color at activation time).
    ///
    /// Fungible with destroy / draw / etc. — runs through `execute()` like any
    /// other action. The CR 605 stack-bypass distinction (mana ability vs.
    /// regular activated ability) is determined statically by inspecting the
    /// enclosing ability's body for any reachable `AddMana`, not by a separate
    /// `AbilityKind` variant.
    AddMana {
        who: Who,
        count: Expr,
        spec: ManaSpec,
    },

    // ── mana payment ─────────────────────────────────────────────────────
    /// Drain `cost` from the controller's mana pool. Symmetric with `AddMana`.
    /// CR 601.2g/h. Pool-based: this is just the *demand*. Mana abilities
    /// (the *supply*) are activated separately by the strategy as ordinary
    /// playable actions; when the pool can't yet pay, the cost driver yields
    /// control back to the strategy to activate more mana.
    PayMana(crate::ManaCost),

    /// Drain `generic` generic mana from the controller's pool, where the
    /// amount is computed at announcement time — the variable-X mana payment
    /// (CR 601.2b). Symmetric with `PayLife { amount: Expr }`. When `generic`
    /// is `Expr::Ctx(Ctx::Var("$x"))` the schema emits an `XMana` decision and
    /// the executor spends whatever the strategy bound. Pool-based like
    /// `PayMana`: a shortfall yields `ManaShortage` and the cost driver yields
    /// to the strategy to make more mana.
    PayManaX {
        generic: Expr,
    },

    // ── planeswalker loyalty ─────────────────────────────────────────────
    /// Activate-cost adjustment to the source's loyalty (CR 606.5). Sets
    /// `pw_activated_this_turn` so each planeswalker activates at most once
    /// per turn (CR 606.3c). `n` is signed: +1 for "+1: …" abilities, −X
    /// for "−X: …".
    LoyaltyAdjust(i32),

    // ── replicate ────────────────────────────────────────────────────────
    /// CR 702.58. Pay `cost` zero-or-more extra times at announcement; each
    /// extra payment creates a copy of the spell on the stack. Only valid
    /// inside a cast cost tree (not an arbitrary effect body).
    Replicate(crate::ManaCost),

    // ── library placement ────────────────────────────────────────────────
    /// Move `count` cards from zone `from` (owned by `who`) onto their
    /// library — `top` = top, `!top` = bottom. Agency: strategy picks which
    /// cards via `state.evaluate_card` (worst first for put-back semantics).
    PutOnLibrary {
        who: Who,
        count: Expr,
        from: ZoneKindSel,
        top: bool,
    },

    // ── noop ─────────────────────────────────────────────────────────────
    Noop,
}

/// What mana an `AddMana` action produces.
#[derive(Clone)]
pub enum ManaSpec {
    /// Fixed colors (e.g. `[Blue]` for Island, `[]` for a colorless source).
    /// If shorter than `count`, the remainder is padded with colorless.
    Fixed(Vec<crate::Color>),
    /// All produced mana is one color, chosen at activation. The chosen color
    /// is read from `BindEnv.chosen_color`. (Lotus Petal, Lion's Eye Diamond,
    /// Mox Opal, Birds of Paradise.)
    AnyOneColor,
}

/// Token specification — kept minimal; grows as token-generating cards land.
#[derive(Clone)]
pub struct TokenSpec {
    pub name: &'static str,
    pub types: Vec<crate::CardType>,
    pub subtypes: Vec<&'static str>,
    pub colors: Vec<crate::Color>,
    pub power: Option<i64>,
    pub toughness: Option<i64>,
    pub keywords: Vec<crate::Keyword>,
}
