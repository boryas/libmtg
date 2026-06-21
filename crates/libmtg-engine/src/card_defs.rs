use std::collections::HashMap;
use std::sync::Arc;
use super::*;
use crate::ir::ability::CostBody;

// ── IR cost helpers ───────────────────────────────────────────────────────────
//
// Tight wrappers for the recurring `CostBody::Ir(Action::…)` shapes used by
// per-card migrations. Each helper builds one structural cost; compose via
// `Sequence` for conjunctions (see `ir_seq`).

fn ir_tap_source() -> CostBody {
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    CostBody::Ir(Action::Tap { target: Expr::Ctx(Ctx::Source) })
}

fn ir_pay_mana_str(s: &str) -> CostBody {
    use crate::ir::action::Action;
    CostBody::Ir(Action::PayMana(parse_mana_cost(s)))
}

fn ir_pay_life(n: i64) -> CostBody {
    use crate::ir::action::{Action, Who};
    use crate::ir::expr::Expr;
    CostBody::Ir(Action::PayLife { who: Who::You, amount: Expr::Num(n) })
}

fn ir_loyalty(n: i32) -> CostBody {
    use crate::ir::action::Action;
    CostBody::Ir(Action::LoyaltyAdjust(n))
}

/// `Sacrifice ~` cost shape — single source, MoveByChoice with `It == Source`.
#[allow(dead_code)]
fn ir_sac_self(bind: &'static str) -> CostBody {
    use crate::ir::action::{Action, MoveVerb, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};
    CostBody::Ir(Action::MoveByChoice {
        who: Who::You,
        from: ZoneKindSel::Battlefield,
        to: ZoneKindSel::Graveyard,
        verb: MoveVerb::Sacrifice,
        filter: Filter(Expr::Eq(
            Box::new(Expr::Ctx(Ctx::It)),
            Box::new(Expr::Ctx(Ctx::Source)),
        )),
        count: Expr::Num(1),
        bind_as: Some(bind),
    })
}

/// Inner action for `Tap source` (no CostBody wrapper) — for use inside Sequence.
fn act_tap_source() -> crate::ir::action::Action {
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    Action::Tap { target: Expr::Ctx(Ctx::Source) }
}

/// Inner Sacrifice~ action.
fn act_sac_self(bind: &'static str) -> crate::ir::action::Action {
    use crate::ir::action::{Action, MoveVerb, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};
    Action::MoveByChoice {
        who: Who::You,
        from: ZoneKindSel::Battlefield,
        to: ZoneKindSel::Graveyard,
        verb: MoveVerb::Sacrifice,
        filter: Filter(Expr::Eq(
            Box::new(Expr::Ctx(Ctx::It)),
            Box::new(Expr::Ctx(Ctx::Source)),
        )),
        count: Expr::Num(1),
        bind_as: Some(bind),
    }
}

fn act_pay_mana_str(s: &str) -> crate::ir::action::Action {
    use crate::ir::action::Action;
    Action::PayMana(parse_mana_cost(s))
}

fn act_pay_life(n: i64) -> crate::ir::action::Action {
    use crate::ir::action::{Action, Who};
    use crate::ir::expr::Expr;
    Action::PayLife { who: Who::You, amount: Expr::Num(n) }
}

/// Wrap a vec of actions in `CostBody::Ir(Action::Sequence(...))`.
fn ir_seq(actions: Vec<crate::ir::action::Action>) -> CostBody {
    use crate::ir::action::Action;
    CostBody::Ir(Action::Sequence(actions))
}

/// `additional_costs` shape for "as an additional cost, pay X generic mana"
/// (Engineered Explosives, Meltdown, Prismatic Ending convergence, etc.). The
/// X amount is the announced `chosen_x`, bound to `$x` at pay time and shared
/// with the spell's resolution effect.
fn ir_xmana_cost() -> CostBody {
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    CostBody::Ir(Action::PayManaX { generic: Expr::Ctx(Ctx::Var("$x")) })
}

/// `additional_costs` shape for "as an additional cost, pay X life"
/// (Toxic Deluge, etc.). Same `$x` binding as [`ir_xmana_cost`].
fn ir_xlife_cost() -> CostBody {
    use crate::ir::action::{Action, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    CostBody::Ir(Action::PayLife { who: Who::You, amount: Expr::Ctx(Ctx::Var("$x")) })
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build the full card catalog used by the simulation engine.
pub fn build_catalog() -> HashMap<String, CardDef> {
    all_cards()
        .into_iter()
        .map(|mut c| {
            // Synthesize a legacy `AbilityDef` for each IR `AbilityKind::Activated`
            // and append it to the kind-specific ability list. This lets the
            // existing `collect_legal_actions` / `run_activate_submachine` pipeline
            // dispatch IR activated abilities without duplicate discovery logic.
            // Synthesized entries carry the IR body via `AbilityDef.ir_body`, which
            // `build_ability_effect` already honors.
            let synthesized: Vec<AbilityDef> = c
                .abilities
                .iter()
                .filter_map(crate::ir::executor::ir_activated_as_legacy)
                .collect();
            if !synthesized.is_empty() {
                if let Some(list) = c.abilities_vec_mut() {
                    list.extend(synthesized);
                }
            }
            // For each IR `AbilityKind::Activated` that classifies as a mana
            // ability per CR 605.1a (no target + body could produce mana),
            // synthesize a legacy `ManaAbility` entry so the existing
            // synchronous mana sub-loop picks it up. `ir_activated_as_legacy`
            // skips these (returns None for mana-classified activated
            // abilities), so each IR ability lands in exactly one list.
            let synthesized_mana: Vec<ManaAbility> = c
                .abilities
                .iter()
                .filter_map(crate::ir::executor::ir_activated_as_mana_ability_legacy)
                .collect();
            if !synthesized_mana.is_empty() {
                if let Some(list) = c.mana_abilities_vec_mut() {
                    list.extend(synthesized_mana);
                }
            }
            (c.name.clone(), c)
        })
        .collect()
}

fn all_cards() -> Vec<CardDef> {
    vec![
        // Lands — basics
        island(),
        swamp(),
        plains(),
        mountain(),
        forest(),
        wastes(),
        snow_covered_island(),
        snow_covered_swamp(),
        snow_covered_plains(),
        snow_covered_mountain(),
        snow_covered_forest(),
        snow_covered_wastes(),
        // Lands — ABU duals
        underground_sea(),
        tundra(),
        badlands(),
        taiga(),
        savannah(),
        scrubland(),
        volcanic_island(),
        bayou(),
        plateau(),
        tropical_island(),
        // Lands — MKM surveil duals (enter tapped)
        undercity_sewers(),
        meticulous_archive(),
        raucous_theater(),
        hedge_maze(),
        commercial_district(),
        lush_portico(),
        thundering_falls(),
        underground_mortuary(),
        elegant_parlor(),
        shadowy_backstreet(),
        // Lands — fetches
        polluted_delta(),
        flooded_strand(),
        misty_rainforest(),
        scalding_tarn(),
        marsh_flats(),
        bloodstained_mire(),
        windswept_heath(),
        wooded_foothills(),
        verdant_catacombs(),
        arid_mesa(),
        // Lands — other
        wasteland(),
        karakas(),
        ancient_tomb(),
        city_of_traitors(),
        cavern_of_souls(),
        urborg_tomb_of_yawgmoth(),
        yavimaya_cradle_of_growth(),
        mistrise_village(),
        great_furnace(),
        // Artifacts
        lotus_petal(),
        lions_eye_diamond(),
        mox_opal(),
        ursas_saga(),
        engineered_explosives(),
        grafdiggers_cage(),
        mishras_bauble(),
        cori_steel_cutter(),
        batterskull(),
        meteor_sword(),
        pre_war_formalwear(),
        cryptic_coat(),
        // Spells — instants
        brainstorm(),
        consider(),
        daze(),
        force_of_negation(),
        force_of_will(),
        dark_ritual(),
        fatal_push(),
        snuff_out(),
        swords_to_plowshares(),
        bitter_triumph(),
        long_goodbye(),
        consign_to_memory(),
        surgical_extraction(),
        lightning_bolt(),
        abrade(),
        red_elemental_blast(),
        pyroblast(),
        blue_elemental_blast(),
        hydroblast(),
        sheoldreds_edict(),
        spell_pierce(),
        stifle(),
        flusterstorm(),
        mindbreak_trap(),
        // Spells — sorceries
        brotherhoods_end(),
        toxic_deluge(),
        doomsday(),
        stock_up(),
        preordain(),
        ponder(),
        thoughtseize(),
        unearth(),
        hymn_to_tourach(),
        edge_of_autumn(),
        personal_tutor(),
        flow_state(),
        green_suns_zenith(),
        show_and_tell(),
        omniscience(),
        sneak_attack(),
        // Creatures
        thassas_oracle(),
        street_wraith(),
        barrowgoyf(),
        ingenious_infiltrator(),
        kaito_bane_of_nightmares(),
        recruiter_of_the_guard(),
        stoneforge_mystic(),
        orcish_bowmasters(),
        murktide_regent(),
        dauthi_voidwalker(),
        lavinia_azorius_renegade(),
        phelia_exuberant_shepherd(),
        hexing_squelcher(),
        dragons_rage_channeler(),
        simian_spirit_guide(),
        fury(),
        quantum_riddler(),
        griselbrand(),
        emrakul_the_aeons_torn(),
        atraxa_grand_unifier(),
        // DFCs / split
        tamiyo_inquisitive_student(),
        brazen_borrower(),
        containment_priest(),
        delver_of_secrets(),
        // Spells — Izzet Delver
        unholy_heat(),
        price_of_progress(),
        meltdown(),
        rough_tumble(),
        prismatic_ending(),
        // Opponent archetypes / hate cards
        null_rod(),
        karn_the_great_creator(),
        painters_servant(),
        leyline_of_the_void(),
        disruptor_flute(),
        blood_moon(),
        magus_of_the_moon(),
        // Tokens
        orc_army_token(),
        clue_token(),
        monk_token(),
        phyrexian_germ_token(),
        mysterious_creature_token(),
    ]
}

// ── Local helpers ─────────────────────────────────────────────────────────────

/// `CardDef` with no supertypes, normal layout, no back, no triggers/replacements/statics.
fn simple(name: &str, kind: CardKind, colors: Vec<Color>, play_weight: Option<u32>) -> CardDef {
    CardDef::new(
        name, kind, colors, play_weight,
        vec![], CardLayout::Normal, None, vec![], vec![], vec![], vec![],
    )
}

/// Convenience: wrap a single target_spec + factory into `Some(SpellModes::Single(...))`.
fn single_mode(
    target_spec: TargetSpec,
    factory: impl Fn(PlayerId, ObjId, u32) -> Effect + Send + Sync + 'static,
) -> Option<SpellModes> {
    Some(SpellModes::Single(SpellMode { target_spec, factory: Arc::new(factory) }))
}

/// Convenience: `single_mode` with `TargetSpec::None`.
fn untargeted_mode(
    factory: impl Fn(PlayerId, ObjId, u32) -> Effect + Send + Sync + 'static,
) -> Option<SpellModes> {
    single_mode(TargetSpec::None, factory)
}

/// IR resolution body for "counter target spell/ability unless its controller
/// pays `cost`" (Daze, Spell Pierce). The spell's controller is offered a choice:
/// pay the tax (the spell then resolves normally — `Noop`), or decline (the spell
/// is countered). The `Choose` executor filters the pay option out when it's
/// unaffordable, and the default strategy takes the first legal option — i.e. pay
/// whenever possible. Mana abilities are auto-tapped during the resolution-time
/// payment (see `pay_ir_cost`).
pub(crate) fn counter_unless_pays_body(cost: ManaCost) -> crate::ir::action::Action {
    use crate::ir::action::{Action, ChoiceOption, Who as IrWho};
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let target = || Expr::Ctx(Ctx::Var("target"));
    Action::Choose {
        who: IrWho::Player(Expr::Controller(Box::new(target()))),
        prompt: "Pay the tax or be countered",
        options: vec![
            ChoiceOption {
                label: "pay",
                cost: Some(Box::new(Action::PayMana(cost))),
                action: Box::new(Action::Noop),
            },
            ChoiceOption {
                label: "be countered",
                cost: None,
                action: Box::new(Action::Counter { target: target() }),
            },
        ],
        bind_as: None,
    }
}

fn color_to_mana_char(c: Color) -> &'static str {
    match c {
        Color::White => "W", Color::Blue => "U", Color::Black => "B",
        Color::Red => "R", Color::Green => "G",
    }
}

/// `ManaAbility` that taps self and produces the given mana string (e.g. `"U"`, `"B"`).
fn tap_produces(s: &str) -> ManaAbility {
    let s_owned = s.to_string();
    ManaAbility {
        costs: ir_tap_source(),
        produces: produces_colors(s),
        produces_count: 1,
        make_effect: std::sync::Arc::new(move |who, _color| eff_mana(who, s_owned.clone())),
        ..Default::default()
    }
}

/// IR mana ability: tap self to produce the mana listed in `s`
/// ("U" = one blue, "CC" = two colorless, "" = one colorless / Wastes).
/// Built as a no-target `AbilityKind::Activated` whose body is `Action::AddMana`;
/// the bridge classifies it as a mana ability via `is_mana_ability` (CR 605.1a).
fn ir_tap_mana(s: &str) -> crate::ir::ability::Ability {
    use crate::ir::ability::{Ability, AbilityKind, CostBody};
    use crate::ir::action::{Action, ManaSpec, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let colors = produces_colors(s);
    let count = if s.is_empty() {
        1 // Wastes: one colorless
    } else {
        s.chars().count() as i64
    };
    Ability {
        kind: AbilityKind::Activated {
            // Tap-self mana ability cost on the IR grammar.
            cost: CostBody::Ir(Action::Tap { target: Expr::Ctx(Ctx::Source) }),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::AddMana {
                who: Who::You,
                count: Expr::Num(count),
                spec: ManaSpec::Fixed(colors),
            },
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("{T}: Add mana."),
    }
}

/// `AbilityDef` for a fetch land: sacrifice self, pay 1 life, search → Battlefield.
fn fetch_ability(filter: crate::ir::expr::Filter) -> AbilityDef {
    AbilityDef {
        costs: ir_seq(vec![act_sac_self("$fetch_self"), act_pay_life(1)]),
        ability_factory: Some(Arc::new(move |who, _| {
            eff_fetch_search(who, filter.clone(), ZoneId::Battlefield)
        })),
        ..Default::default()
    }
}

/// Basic land (Island, Swamp, Plains, Mountain, Forest, Wastes).
fn basic_land(name: &str, land_types: LandTypes, mana: &str) -> CardDef {
    let mut def = CardDef::new(
        name, CardKind::Land(LandData {
            land_types,
            mana_abilities: vec![],
            ..Default::default()
        }),
        vec![], Some(25), vec![Supertype::Basic], CardLayout::Normal, None,
        vec![], vec![], vec![], vec![],
    );
    def.abilities.push(ir_tap_mana(mana));
    def
}

/// Basic snow land (Snow-Covered X).
fn snow_basic(name: &str, land_types: LandTypes, mana: &str) -> CardDef {
    let mut def = CardDef::new(
        name, CardKind::Land(LandData {
            land_types,
            mana_abilities: vec![],
            ..Default::default()
        }),
        vec![], Some(25), vec![Supertype::Basic, Supertype::Snow], CardLayout::Normal, None,
        vec![], vec![], vec![], vec![],
    );
    def.abilities.push(ir_tap_mana(mana));
    def
}

/// Dual land that always enters tapped (surveil lands, etc.).
/// MKM-style surveil dual: always enters tapped, triggers surveil 1 on ETB.
fn surveil_dual(name: &'static str, land_types: LandTypes, c1: &str, c2: &str) -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, EventPattern, ReplacementBody};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};

    let trigger = etb_self_trigger(name, TargetSpec::None, move |_, controller| {
        eff_surveil(controller, 1)
    });
    let self_etb = Filter(Expr::Eq(
        Box::new(Expr::Ctx(Ctx::It)),
        Box::new(Expr::Ctx(Ctx::Source)),
    ));
    let enters_tapped = Ability {
        kind: AbilityKind::Replacement {
            matches: EventPattern::EntersZone {
                obj_filter: self_etb,
                zone_kind: ZoneKindSel::Battlefield,
            },
            condition: None,
            body: ReplacementBody::Replace(Action::Sequence(vec![
                Action::Move {
                    what: Expr::Ctx(Ctx::Var("triggered_obj")),
                    to: ZoneKindSel::Battlefield,
                    to_owner: None,
                    bind_as: None,
                },
                Action::Tap { target: Expr::Ctx(Ctx::Var("triggered_obj")) },
            ])),
        },
        text: Some("~ enters tapped."),
    };
    let mut card = CardDef::new(
        name,
        CardKind::Land(LandData {
            land_types,
            mana_abilities: vec![tap_produces(c1), tap_produces(c2)],
            ..Default::default()
        }),
        vec![], None, vec![], CardLayout::Normal, None,
        vec![trigger],
        vec![],
        vec![],
        vec![],
    );
    card.abilities = vec![enters_tapped];
    card
}

// ── Lands ─────────────────────────────────────────────────────────────────────

/// ABU dual land: two basic land subtypes, two IR tap-mana abilities (one per color).
fn abu_dual(name: &str, a: BasicLandType, b: BasicLandType, ma: &str, mb: &str) -> CardDef {
    let mut def = simple(name, CardKind::Land(LandData {
        land_types: LandTypes::from_types(&[a, b]),
        mana_abilities: vec![],
        ..Default::default()
    }), vec![], None);
    def.abilities.push(ir_tap_mana(ma));
    def.abilities.push(ir_tap_mana(mb));
    def
}

fn underground_sea()  -> CardDef { abu_dual("Underground Sea",  BasicLandType::Island,   BasicLandType::Swamp,    "U", "B") }
fn tundra()           -> CardDef { abu_dual("Tundra",           BasicLandType::Plains,   BasicLandType::Island,   "W", "U") }
fn badlands()         -> CardDef { abu_dual("Badlands",         BasicLandType::Swamp,    BasicLandType::Mountain, "B", "R") }
fn taiga()            -> CardDef { abu_dual("Taiga",            BasicLandType::Mountain, BasicLandType::Forest,   "R", "G") }
fn savannah()         -> CardDef { abu_dual("Savannah",         BasicLandType::Forest,   BasicLandType::Plains,   "G", "W") }
fn scrubland()        -> CardDef { abu_dual("Scrubland",        BasicLandType::Plains,   BasicLandType::Swamp,    "W", "B") }
fn volcanic_island()  -> CardDef { abu_dual("Volcanic Island",  BasicLandType::Island,   BasicLandType::Mountain, "U", "R") }
fn bayou()            -> CardDef { abu_dual("Bayou",            BasicLandType::Swamp,    BasicLandType::Forest,   "B", "G") }
fn plateau()          -> CardDef { abu_dual("Plateau",          BasicLandType::Mountain, BasicLandType::Plains,   "R", "W") }
fn tropical_island()  -> CardDef { abu_dual("Tropical Island",  BasicLandType::Forest,   BasicLandType::Island,   "G", "U") }

fn swamp() -> CardDef {
    let mut def = CardDef::new(
        "Swamp",
        CardKind::Land(LandData {
            land_types: LandTypes::from_types(&[BasicLandType::Swamp]),
            mana_abilities: vec![],
            ..Default::default()
        }),
        vec![], Some(25), vec![Supertype::Basic], CardLayout::Normal, None,
        vec![], vec![], vec![], vec![],
    );
    def.abilities.push(ir_tap_mana("B"));
    def
}

fn island() -> CardDef {
    let mut def = CardDef::new(
        "Island",
        CardKind::Land(LandData {
            land_types: LandTypes::from_types(&[BasicLandType::Island]),
            mana_abilities: vec![],
            ..Default::default()
        }),
        vec![], Some(25), vec![Supertype::Basic], CardLayout::Normal, None,
        vec![], vec![], vec![], vec![],
    );
    def.abilities.push(ir_tap_mana("U"));
    def
}

fn plains() -> CardDef {
    basic_land("Plains", LandTypes::from_types(&[BasicLandType::Plains]), "W")
}

fn mountain() -> CardDef {
    basic_land("Mountain", LandTypes::from_types(&[BasicLandType::Mountain]), "R")
}

fn forest() -> CardDef {
    basic_land("Forest", LandTypes::from_types(&[BasicLandType::Forest]), "G")
}

/// Wastes: basic land with no subtype, produces {C}.
fn wastes() -> CardDef {
    basic_land("Wastes", LandTypes::default(), "")
}

fn snow_covered_island() -> CardDef {
    snow_basic("Snow-Covered Island", LandTypes::from_types(&[BasicLandType::Island]), "U")
}

fn snow_covered_swamp() -> CardDef {
    snow_basic("Snow-Covered Swamp", LandTypes::from_types(&[BasicLandType::Swamp]), "B")
}

fn snow_covered_plains() -> CardDef {
    snow_basic("Snow-Covered Plains", LandTypes::from_types(&[BasicLandType::Plains]), "W")
}

fn snow_covered_mountain() -> CardDef {
    snow_basic("Snow-Covered Mountain", LandTypes::from_types(&[BasicLandType::Mountain]), "R")
}

fn snow_covered_forest() -> CardDef {
    snow_basic("Snow-Covered Forest", LandTypes::from_types(&[BasicLandType::Forest]), "G")
}

fn snow_covered_wastes() -> CardDef {
    let mut def = CardDef::new(
        "Snow-Covered Wastes",
        CardKind::Land(LandData {
            land_types: LandTypes::default(),
            mana_abilities: vec![],
            ..Default::default()
        }),
        vec![], Some(25), vec![Supertype::Basic, Supertype::Snow], CardLayout::Normal, None,
        vec![], vec![], vec![], vec![],
    );
    def.abilities.push(ir_tap_mana(""));
    def
}

/// Enters tapped. CR 614.1 (replacement effect): replaces the ETB event to set tapped=true.
// ── MKM surveil lands (always enter tapped; surveil 1 on ETB) ─────────────────

fn undercity_sewers()     -> CardDef { surveil_dual("Undercity Sewers",     LandTypes::from_types(&[BasicLandType::Island, BasicLandType::Swamp]), "U", "B") }
fn meticulous_archive()   -> CardDef { surveil_dual("Meticulous Archive",   LandTypes::from_types(&[BasicLandType::Plains, BasicLandType::Island]), "W", "U") }
fn raucous_theater()      -> CardDef { surveil_dual("Raucous Theater",      LandTypes::from_types(&[BasicLandType::Swamp, BasicLandType::Mountain]), "B", "R") }
fn hedge_maze()           -> CardDef { surveil_dual("Hedge Maze",           LandTypes::from_types(&[BasicLandType::Mountain, BasicLandType::Forest]), "R", "G") }
fn commercial_district()  -> CardDef { surveil_dual("Commercial District",  LandTypes::from_types(&[BasicLandType::Forest, BasicLandType::Plains]), "G", "W") }
fn lush_portico()         -> CardDef { surveil_dual("Lush Portico",         LandTypes::from_types(&[BasicLandType::Plains, BasicLandType::Forest]), "W", "G") }
fn thundering_falls()     -> CardDef { surveil_dual("Thundering Falls",     LandTypes::from_types(&[BasicLandType::Island, BasicLandType::Mountain]), "U", "R") }
fn underground_mortuary() -> CardDef { surveil_dual("Underground Mortuary", LandTypes::from_types(&[BasicLandType::Swamp, BasicLandType::Forest]), "B", "G") }
fn elegant_parlor()       -> CardDef { surveil_dual("Elegant Parlor",       LandTypes::from_types(&[BasicLandType::Mountain, BasicLandType::Plains]), "R", "W") }
fn shadowy_backstreet()   -> CardDef { surveil_dual("Shadowy Backstreet",   LandTypes::from_types(&[BasicLandType::Plains, BasicLandType::Swamp]), "W", "B") }

/// {T}, Sacrifice: destroy target nonbasic land. CR 701.7.
fn wasteland() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, CostBody};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};

    let nonbasic_land = ir_and(
        ir_type(CardType::Land),
        ir_not(ir_supertype(Supertype::Basic)),
    );
    let mut card = simple(
        "Wasteland",
        CardKind::Land(LandData::default()),
        vec![],
        None,
    );
    card.abilities = vec![Ability {
        kind: AbilityKind::Activated {
            // Phase 4 step 3: TapSelf+SacSelf conjunction migrated to IR.
            // Lowered back to legacy by the Sequence-aware shim arm.
            cost: CostBody::Ir(Action::Sequence(vec![
                Action::Tap { target: Expr::Ctx(Ctx::Source) },
                Action::Sacrifice {
                    who: crate::ir::action::Who::You,
                    filter: Filter(Expr::Eq(
                        Box::new(Expr::Ctx(Ctx::It)),
                        Box::new(Expr::Ctx(Ctx::Source)),
                    )),
                    count: Expr::Num(1),
                    bind_as: None,
                },
            ])),
            target_spec: TargetSpec::ObjectInZone {
                controller: Who::Opp,
                zone: ZoneId::Battlefield,
                filter: nonbasic_land,
            },
            choice_spec: None,
            body: Action::Destroy {
                target: Expr::Ctx(Ctx::Var("target")),
            },
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: crate::ir::expr::ZoneKindSel::Battlefield,
        },
        text: Some("{T}, Sacrifice Wasteland: Destroy target nonbasic land."),
    }];
    card
}

fn karakas() -> CardDef {
    let legend_creature = ir_and(
        ir_type(CardType::Creature),
        ir_supertype(Supertype::Legendary),
    );
    CardDef::new(
        "Karakas",
        CardKind::Land(LandData {
            mana_abilities: vec![tap_produces("W")],
            abilities: vec![AbilityDef {
                costs: ir_tap_source(),
                target_spec: TargetSpec::Union(vec![
                    TargetSpec::ObjectInZone {
                        controller: Who::Actor,
                        zone: ZoneId::Battlefield,
                        filter: legend_creature.clone(),
                    },
                    TargetSpec::ObjectInZone {
                        controller: Who::Opp,
                        zone: ZoneId::Battlefield,
                        filter: legend_creature,
                    },
                ]),
                ability_factory: Some(Arc::new(|who, _| eff_bounce_target(who))),
                ..Default::default()
            }],
            ..Default::default()
        }),
        vec![], None, vec![Supertype::Legendary], CardLayout::Normal, None,
        vec![], vec![], vec![], vec![],
    )
}

fn ancient_tomb() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, CostBody};
    use crate::ir::action::{Action, ManaSpec, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut def = simple("Ancient Tomb", CardKind::Land(LandData {
        mana_abilities: vec![],
        ..Default::default()
    }), vec![], None);
    def.abilities.push(Ability {
        kind: AbilityKind::Activated {
            // Same cost shape as Underground Sea's tap-for-mana abilities
            // (`{T}: Add <X>`). Body differs — adds CC and pays 2 life as
            // a side-effect — but the cost is just TapSelf.
            cost: CostBody::Ir(Action::Tap { target: Expr::Ctx(Ctx::Source) }),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::Sequence(vec![
                Action::AddMana {
                    who: Who::You,
                    count: Expr::Num(2),
                    spec: ManaSpec::Fixed(vec![]), // CC — pad with colorless
                },
                Action::PayLife { who: Who::You, amount: Expr::Num(2) },
            ]),
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("{T}: Add {C}{C}. Ancient Tomb deals 2 damage to you."),
    });
    def
}

fn city_of_traitors() -> CardDef {
    CardDef::new(
        "City of Traitors",
        CardKind::Land(LandData {
            mana_abilities: vec![ManaAbility {
                costs: ir_tap_source(),
                produces_count: 2,
                make_effect: Arc::new(|who, _| eff_mana(who, "CC")),
                ..Default::default()
            }],
            ..Default::default()
        }),
        vec![], None,
        vec![], CardLayout::Normal, None,
        vec![TriggerDef {
            check: Arc::new(|event, source_id, controller, _state, pending| {
                // "When you play another land, sacrifice City of Traitors."
                if let GameEvent::LandPlayed { id, controller: ctlr } = event {
                    if *id != source_id && *ctlr == controller {
                        pending.push(TriggerContext {
                            source_name: "City of Traitors".into(),
                            controller,
                            target_spec: TargetSpec::None,
                            effect: Effect(Arc::new(move |state, t, _targets| {
                                if state.permanent_bf(source_id).is_some() {
                                    state.log(t, controller, "City of Traitors → sacrifice (another land played)");
                                    change_zone(source_id, ZoneId::Graveyard, state, t, controller);
                                }
                            })),
                        });
                    }
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![], vec![],
    )
}

fn polluted_delta() -> CardDef {
    simple("Polluted Delta", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_or(ir_subtype("island"), ir_subtype("swamp")),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn flooded_strand() -> CardDef {
    simple("Flooded Strand", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_subtype("island"),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn misty_rainforest() -> CardDef {
    simple("Misty Rainforest", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_subtype("island"),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn scalding_tarn() -> CardDef {
    simple("Scalding Tarn", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_subtype("island"),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn marsh_flats() -> CardDef {
    simple("Marsh Flats", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_subtype("swamp"),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn bloodstained_mire() -> CardDef {
    simple("Bloodstained Mire", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_subtype("swamp"),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn windswept_heath() -> CardDef {
    simple("Windswept Heath", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_or(ir_subtype("forest"), ir_subtype("plains")),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn wooded_foothills() -> CardDef {
    simple("Wooded Foothills", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_or(ir_subtype("forest"), ir_subtype("mountain")),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn verdant_catacombs() -> CardDef {
    simple("Verdant Catacombs", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_or(ir_subtype("forest"), ir_subtype("swamp")),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

fn arid_mesa() -> CardDef {
    simple("Arid Mesa", CardKind::Land(LandData {
        abilities: vec![fetch_ability(ir_and(
            ir_type(CardType::Land),
            ir_or(ir_subtype("plains"), ir_subtype("mountain")),
        ))],
        ..Default::default()
    }), vec![], Some(25))
}

/// Produces generic mana only (no colored pips). CR 106.
/// Legendary land. ETB: choose a creature type (logged, used for future uncounterable modeling).
/// {T}: Add {C}.
/// {T}: Add one mana of any color (TODO: restrict to spells of the named type; mana is uncounterable).
fn cavern_of_souls() -> CardDef {
    let repl = etb_self_replacement(|source_id, id, controller, state, t| {
        let ChoiceResult::CreatureType(chosen_type) =
            state.with_strategy(controller, |s, st| s.resolve_choice(source_id, &ChoiceRequest::CreatureType, st)) else { return };
        if let Some(bf) = state.permanent_bf_mut(id) {
            bf.etb_choice = Some(ChoiceResult::CreatureType(chosen_type.clone()));
        }
        state.log(t, controller, format!("Cavern of Souls names \"{}\"", chosen_type));
    });
    CardDef::new(
        "Cavern of Souls",
        CardKind::Land(LandData {
            // {T}: Add {C} — colorless
            // {T}: Add one mana of any color (type restriction and uncounterable not yet modeled)
            mana_abilities: vec![
                ManaAbility {
                    costs: ir_tap_source(),
                    make_effect: std::sync::Arc::new(|who, _| eff_mana(who, "C")),
                    ..Default::default()
                },
                ManaAbility {
                    costs: ir_tap_source(),
                    produces: produces_colors("WUBRG"),
                    make_effect: std::sync::Arc::new(|who, color| {
                        eff_mana(who, color.map(color_to_mana_char).unwrap_or("1"))
                    }),
                    // Colored mana only for creature spells of the named type (CR 106).
                    // Creature-type matching is coarsened to "is creature" since
                    // the sim doesn't track per-card creature subtypes. The
                    // gate is "the spell being cast is a creature" — a Filter
                    // over `GameCtx::CastingSpell`, not the source land.
                    condition: Some(crate::ir::expr::Filter(crate::ir::expr::Expr::Contains(
                        Box::new(crate::ir::expr::Expr::TypeLit(CardType::Creature)),
                        Box::new(crate::ir::expr::Expr::Types(Box::new(
                            crate::ir::expr::Expr::GameCtx(crate::ir::context::GameCtx::CastingSpell),
                        ))),
                    ))),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        vec![],
        Some(50),
        vec![Supertype::Legendary], CardLayout::Normal, None,
        vec![],
        vec![repl],
        vec![],
        vec![],
    )
}

// ── Artifacts ─────────────────────────────────────────────────────────────────

/// Sacrifice: add one mana of any color. CR 106.3.
fn lotus_petal() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, CostBody};
    use crate::ir::action::{Action, ManaSpec, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};
    let mut def = simple("Lotus Petal", CardKind::Artifact(ArtifactData {
        mana_cost: "0".to_string(),
        mana_abilities: vec![],
        ..Default::default()
    }), vec![], Some(25));
    def.abilities.push(Ability {
        kind: AbilityKind::Activated {
            // Sacrifice-self cost on the IR grammar. Filter is `It == Source`
            // — the canonical "this object only" shape.
            cost: CostBody::Ir(Action::Sacrifice {
                who: Who::You,
                filter: Filter(Expr::Eq(
                    Box::new(Expr::Ctx(Ctx::It)),
                    Box::new(Expr::Ctx(Ctx::Source)),
                )),
                count: Expr::Num(1),
                bind_as: None,
            }),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::AddMana {
                who: Who::You,
                count: Expr::Num(1),
                spec: ManaSpec::AnyOneColor,
            },
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("Sacrifice Lotus Petal: Add one mana of any color."),
    });
    def
}

/// Discard your hand, Sacrifice Lion's Eye Diamond: Add three mana of any one color.
/// Activate only as an instant. CR 605.3, CR 601.2g (excluded from mana sub-loop).
fn lions_eye_diamond() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::action::{Action, ManaSpec, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut def = simple("Lion's Eye Diamond", CardKind::Artifact(ArtifactData {
        mana_cost: "0".to_string(),
        mana_abilities: vec![],
        ..Default::default()
    }), vec![], Some(10));
    def.abilities.push(Ability {
        kind: AbilityKind::Activated {
            // "Discard your hand, Sacrifice ~". The discard side is modelled
            // as `Action::Discard` with `count = HandSize(Controller)` —
            // the "Discard(All)" composition (per
            // feedback_discard_hand_idiom.md): no separate `DiscardHand`
            // primitive, just a dynamic count over the canonical hand-size
            // expression. The walk emits no decision (dynamic count) and
            // the executor's loop sweeps the hand.
            cost: ir_seq(vec![
                Action::Discard {
                    who: Who::You,
                    count: Expr::HandSize(Box::new(Expr::Ctx(Ctx::Controller))),
                    at_random: false,
                    filter: None,
                },
                act_sac_self("$led_self"),
            ]),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::AddMana {
                who: Who::You,
                count: Expr::Num(3),
                spec: ManaSpec::AnyOneColor,
            },
            timing: ActivationTiming::Instant,
            activation_condition: None,
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("Discard your hand, Sacrifice: Add three mana of any one color."),
    });
    def
}

/// Mox Opal — Legendary Artifact, {0}.
/// Metalcraft — {T}: Add one mana of any color. Activate only if you control three or more artifacts.
fn mox_opal() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::action::{Action, ManaSpec, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel, ZoneSel};
    // Metalcraft: count of artifacts controlled by source's controller >= 3.
    let metalcraft = Expr::Ge(
        Box::new(Expr::Count(Box::new(Expr::AllObjects {
            zone: ZoneSel::Global(ZoneKindSel::Battlefield),
            bind: "a",
            filter: Box::new(Expr::And(
                Box::new(Expr::Eq(
                    Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::Var("a"))))),
                    Box::new(Expr::Ctx(Ctx::Controller)),
                )),
                Box::new(Expr::Contains(
                    Box::new(Expr::TypeLit(CardType::Artifact)),
                    Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::Var("a"))))),
                )),
            )),
        }))),
        Box::new(Expr::Num(3)),
    );
    let mut def = simple("Mox Opal", CardKind::Artifact(ArtifactData {
        mana_cost: "0".to_string(),
        mana_abilities: vec![],
        ..Default::default()
    }), vec![], Some(20));
    def.supertypes.push(Supertype::Legendary);
    def.abilities.push(Ability {
        kind: AbilityKind::Activated {
            cost: ir_tap_source(),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::AddMana {
                who: Who::You,
                count: Expr::Num(1),
                spec: ManaSpec::AnyOneColor,
            },
            timing: ActivationTiming::Default,
            activation_condition: Some(metalcraft),
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("Metalcraft — {T}: Add one mana of any color."),
    });
    def
}

/// **ABNORMAL — sagas are not implemented.** Urza's Saga is a saga; its real
/// behavior is "add a lore counter on entry and after your draw step; chapter
/// abilities trigger when the lore-counter passes thresholds; sacrifice as
/// SBA when lore-count > final chapter (CR 715)." This card stub fakes
/// chapter III as a sacrifice-self *activated* ability, which is wrong shape
/// (the sac is automatic, not paid by the controller; the chapter trigger is
/// not an activated ability). Do not use this stub as a model for any cost
/// migration — see CARD_INDEX.org "Sagas" gap entry. Replace once the saga
/// trigger/lore-counter machinery lands.
///
/// Cost storage is migrated to IR (same broken sac-self shape) so the catalog
/// has zero `CostBody::Legacy` users for sac-shaped costs. Tests that exercise
/// the chapter III predicate (`test_urza_saga_*`) call `eff_fetch_search`
/// directly — they never instantiate this card, so the storage shape is
/// irrelevant to test parity.
fn ursas_saga() -> CardDef {
    let pred = ir_and(
        ir_type(CardType::Artifact),
        ir_and(ir_colorless(), ir_mv_le(1)),
    );
    simple("Urza's Saga", CardKind::Artifact(ArtifactData {
        mana_cost: String::new(),
        abilities: vec![AbilityDef {
            costs: ir_sac_self("$saga_self"),
            ability_factory: Some(Arc::new(move |who, _| {
                eff_fetch_search(who, pred.clone(), ZoneId::Battlefield)
            })),
            ..Default::default()
        }],
        ..Default::default()
    }), vec![], None)
}

/// {1}. Static: creature cards in graveyards and libraries can't enter the battlefield.
/// Players can't cast spells from graveyards or libraries.
/// Two static effects while on the battlefield:
///   (a) CR 614.17 prohibition: creature cards from graveyards/libraries can't enter the BF.
///       Implemented as a `ProhibitionDef` — checked in `fire_event` before replacements.
///   (b) Static CE sets `castable = false` on all cards in graveyard/library zones.
/// Sunburst: enters with a charge counter for each distinct color of mana spent to cast it.
/// Modeled via the IR `PayManaX` additional cost (strategy declares the intended
/// distinct-color count as chosen_x; the engine pays that many generic mana). The
/// ETB replacement reads `resolving_costs_ctx.chosen_x` and places that many Charge counters.
/// {2}, Sacrifice: destroy each nonland permanent with MV equal to the charge count.
/// CR 702.43 sunburst, CR 701.7 destroy.
fn engineered_explosives() -> CardDef {
    use crate::ir::action::{Action, MoveVerb};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};
    let mut def = CardDef::new(
        "Engineered Explosives",
        CardKind::Artifact(ArtifactData {
            mana_cost: "0".to_string(),
            abilities: vec![AbilityDef {
                source_zone: SourceZone::Battlefield,
                // First end-to-end IR migration of an `AbilityDef` cost:
                // `{2}, Sacrifice ~`. Pays through `pay_ir_cost` via the
                // `pay_ability_cost` dispatch on `CostBody`. Cost shape is
                // a Sequence of PayMana(2) and the unified MoveByChoice
                // (Battlefield → Graveyard, verb=Sacrifice) with the
                // `It == Source` filter. PayMana drains the pre-tapped
                // pool (the mana sub-loop has already filled it pre-pay).
                costs: CostBody::Ir(Action::Sequence(vec![
                    Action::PayMana(parse_mana_cost("2")),
                    Action::MoveByChoice {
                        who: crate::ir::action::Who::You,
                        from: ZoneKindSel::Battlefield,
                        to: ZoneKindSel::Graveyard,
                        verb: MoveVerb::Sacrifice,
                        filter: Filter(Expr::Eq(
                            Box::new(Expr::Ctx(Ctx::It)),
                            Box::new(Expr::Ctx(Ctx::Source)),
                        )),
                        count: Expr::Num(1),
                        bind_as: Some("$ee_self"),
                    },
                ])),
                ability_factory: Some(Arc::new(|who, source_id| {
                    // EE has been sacrificed; its charge counters persist in the
                    // objects map, read via `CountersOn(Source)`. Destroy each
                    // nonland permanent whose MV equals that count.
                    eff_ir_targeted(who, source_id, ir_for_each_on_battlefield(
                        ir_and(
                            ir_not(ir_type(CardType::Land)),
                            ir_mv_eq_expr(Expr::CountersOn(
                                Box::new(Expr::Ctx(Ctx::Source)), CounterType::Charge)),
                        ),
                        Action::Destroy { target: Expr::Ctx(Ctx::Var("v")) },
                    ))
                })),
                ..Default::default()
            }],
            ..Default::default()
        }),
        vec![],
        None,
        vec![], CardLayout::Normal, None,
        vec![],
        vec![ReplacementDef {
            check: Arc::new(etb_self_check),
            make_effect: Arc::new(|_source_id, controller: PlayerId| {
                Effect(Arc::new(move |state, t, targets| {
                    let Some(&id) = targets.first() else { return; };
                    let chosen_x = state.resolving_costs_ctx.chosen_x;
                    if let Some(obj) = state.objects.get_mut(&id) {
                        *obj.counters.entry(CounterType::Charge).or_insert(0) = chosen_x;
                    }
                    let from = current_zone_id(id, state);
                    fire_event(
                        GameEvent::ZoneChange { id, actor: controller, from, to: ZoneId::Battlefield, controller },
                        state, t, controller,
                    );
                }))
            }),
            active_when: tp_always(),
        }],
        vec![],
        vec![],
    );
    def.additional_costs = ir_xmana_cost();
    def
}

fn grafdiggers_cage() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, ActionKind, EventPattern};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};

    let mut card = CardDef::new(
        "Grafdigger's Cage",
        CardKind::Artifact(ArtifactData {
            mana_cost: "1".to_string(),
            ..Default::default()
        }),
        vec![],
        Some(40),
        vec![], CardLayout::Normal, None,
        vec![], // no trigger_defs
        vec![], // no replacements
        vec![], // "creatures can't ETB from GY/library" is now an IR Prohibition (below)
        vec![], // "can't cast from GY/library" is now an IR Restriction (below)
    );

    // (a) "Creatures can't enter the battlefield from graveyards or libraries."
    // An IR Prohibition consulted in the event pipeline (fire_event Stage 1): it
    // matches a ZoneChange to the battlefield whose source zone is GY *or* library,
    // for a creature card, and suppresses it (CR 614.17 "can't"). `active_zone`
    // Battlefield gates it to while the Cage is in play; the `Or` keeps the two
    // source zones as one CR ability.
    let creature_obj = || Filter(Expr::Contains(
        Box::new(Expr::TypeLit(CardType::Creature)),
        Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::It)))),
    ));
    let cant_etb = Ability {
        kind: AbilityKind::Prohibition {
            matches: EventPattern::Or(vec![
                EventPattern::ZoneChange {
                    obj_filter: creature_obj(),
                    from: ZoneKindSel::Graveyard,
                    to: ZoneKindSel::Battlefield,
                    actor_filter: None,
                },
                EventPattern::ZoneChange {
                    obj_filter: creature_obj(),
                    from: ZoneKindSel::Library,
                    to: ZoneKindSel::Battlefield,
                    actor_filter: None,
                },
            ]),
            active_zone: Some(ZoneKindSel::Battlefield),
        },
        text: Some("Creatures can't enter the battlefield from graveyards or libraries."),
    };

    // (b) "Players can't cast spells from graveyards or libraries."
    // An action-Restriction consulted at legal-cast enumeration (AND-NOT over
    // castable → "can't beats can", CR 101.2). Zone-scoped subject: any card whose
    // current zone is GY or library — so a Dauthi exile-cast (exile ≠ GY/library)
    // falls out naturally, while flashback (GY) is correctly forbidden.
    let zone_of_it = || Expr::ZoneOf(Box::new(Expr::Ctx(Ctx::It)));
    let cant_cast = Ability {
        kind: AbilityKind::Restriction {
            action: ActionKind::Cast,
            subject: Filter(Expr::Or(
                Box::new(Expr::Eq(
                    Box::new(zone_of_it()),
                    Box::new(Expr::ZoneLit(ZoneId::Graveyard)),
                )),
                Box::new(Expr::Eq(
                    Box::new(zone_of_it()),
                    Box::new(Expr::ZoneLit(ZoneId::Library)),
                )),
            )),
        },
        text: Some("Players can't cast spells from graveyards or libraries."),
    };

    card.abilities = vec![cant_etb, cant_cast];
    card
}

// ── Instants ──────────────────────────────────────────────────────────────────

/// Draw 3, put back 2 (evaluator-driven: puts back the two worst cards).
/// CR 420 (draw), CR 701.26 (library manipulation).
fn brainstorm() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::{Expr, ZoneKindSel};

    let mut card = simple("Brainstorm", CardKind::Instant(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Sequence(vec![
                    Action::Draw { who: IrWho::You, n: Expr::Num(3) },
                    Action::PutOnLibrary {
                        who: IrWho::You,
                        count: Expr::Num(2),
                        from: ZoneKindSel::Hand,
                        top: true,
                    },
                ]),
            }],
        },
        text: Some("Draw three cards, then put two cards from your hand on top of your library in any order."),
    }];
    card
}

/// Surveil 1, then draw 1. CR 701.43 (surveil).
fn consider() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::Expr;
    let mut card = simple("Consider", CardKind::Instant(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Sequence(vec![
                    Action::Surveil { who: IrWho::You, n: Expr::Num(1) },
                    Action::Draw { who: IrWho::You, n: Expr::Num(1) },
                ]),
            }],
        },
        text: Some("Surveil 1, then draw a card."),
    }];
    card
}

/// Counter target spell. Alternate costs: bounce a blue-producing island (free),
/// or pay {1U} (20% probability). CR 701.5.
/// "Counter target spell unless its controller pays {1}."
fn daze() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, MoveVerb};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};
    let mut c = simple("Daze", CardKind::Instant(SpellData {
        mana_cost: "1U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("1U", true, false), None);
    c.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone { controller: Who::Opp, zone: ZoneId::Stack, filter: ir_spell() },
                body: counter_unless_pays_body(parse_mana_cost("1")),
            }],
        },
        text: Some("Counter target spell unless its controller pays {1}."),
    }];
    // Phase 4 step 5 (alt-cost migration): "Return an Island you control to
    // its owner's hand." First card to actually flow through `pay_ir_cost`
    // at runtime. The schema decision is bound to "$daze_island" and the
    // executor reads that binding to know which permanent to bounce.
    let island_filter = Filter(Expr::Contains(
        Box::new(Expr::SubtypeLit("island".to_string())),
        Box::new(Expr::Subtypes(Box::new(Expr::Ctx(Ctx::It)))),
    ));
    c.alternate_costs = vec![
        AlternateCost {
            costs: CostBody::Ir(Action::MoveByChoice {
                who: crate::ir::action::Who::You,
                from: ZoneKindSel::Battlefield,
                to: ZoneKindSel::Hand,
                verb: MoveVerb::Return,
                filter: island_filter,
                count: Expr::Num(1),
                bind_as: Some("$daze_island"),
            }),
            ..Default::default()
        },
    ];
    c
}

/// Filter: "a card of the given color in hand other than the source" — the
/// canonical pitch-cost shape used by FoW, FoN, Fury, and similar Modern
/// Horizons / Mercadian Masques pitch cards. Excluding `Source` enforces
/// "you can't pitch the spell to itself."
fn pitch_color_filter(color: Color) -> crate::ir::expr::Filter {
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};
    Filter(Expr::And(
        Box::new(Expr::Not(Box::new(Expr::Eq(
            Box::new(Expr::Ctx(Ctx::It)),
            Box::new(Expr::Ctx(Ctx::Source)),
        )))),
        Box::new(Expr::Contains(
            Box::new(Expr::ColorLit(color)),
            Box::new(Expr::Colors(Box::new(Expr::Ctx(Ctx::It)))),
        )),
    ))
}

fn pitch_blue_filter() -> crate::ir::expr::Filter {
    pitch_color_filter(Color::Blue)
}

/// Counter target noncreature spell. Pitch cost (exile a blue card) only available when it's
/// not your turn; the countered spell is exiled via a scoped replacement (CR 118.9b, 614.1a).
fn force_of_negation() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, MoveVerb};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut c = simple("Force of Negation", CardKind::Instant(SpellData {
        mana_cost: "1UU".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("1UU", true, false), None);
    // "Counter target noncreature spell." This engine exiles the countered spell
    // instead of leaving it in the graveyard (deliberate model with tests):
    // `Counter` sends it to the graveyard, then `Exile` moves it on to exile.
    c.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Stack,
                    filter: ir_and(ir_spell(), ir_not(ir_type(CardType::Creature))),
                },
                body: Action::Sequence(vec![
                    Action::Counter { target: Expr::Ctx(Ctx::Var("target")) },
                    Action::Exile { target: Expr::Ctx(Ctx::Var("target")), bind_as: None },
                ]),
            }],
        },
        text: Some("Counter target noncreature spell. (Exiled rather than put into the graveyard.)"),
    }];
    // Phase 4 step 5 follow-up: pitch alt cost migrated to MoveByChoice
    // (Hand → Exile, verb=Exile). The hand_min and condition gates are
    // unchanged — those live on AlternateCost, not the cost tree.
    c.alternate_costs = vec![
        AlternateCost {
            costs: CostBody::Ir(Action::MoveByChoice {
                who: crate::ir::action::Who::You,
                from: ZoneKindSel::Hand,
                to: ZoneKindSel::Exile,
                verb: MoveVerb::Exile,
                filter: pitch_blue_filter(),
                count: Expr::Num(1),
                bind_as: Some("$fon_pitch"),
            }),
            hand_min: 2,
            condition: Some(std::sync::Arc::new(|caster, state| {
                state.current_ap != state.player_id(caster)
            })),
            ..Default::default()
        },
    ];
    c
}

/// Counter target spell. Alternate costs: exile a blue card from hand + pay 1 life (pitch),
/// or pay {3UU} (hard cost, rare). CR 702.14 (pitch cost), CR 701.5.
fn force_of_will() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, MoveVerb};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut c = simple("Force of Will", CardKind::Instant(SpellData {
        mana_cost: "3UU".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("3UU", true, false), None);
    c.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Stack,
                    filter: ir_any(),
                },
                body: Action::Counter { target: Expr::Ctx(Ctx::Var("target")) },
            }],
        },
        text: Some("Counter target spell."),
    }];
    // Phase 4 step 5 follow-up: pitch alt cost migrated to a Sequence of
    // MoveByChoice (hand → exile) and PayLife(1).
    c.alternate_costs = vec![
        AlternateCost {
            costs: CostBody::Ir(Action::Sequence(vec![
                Action::MoveByChoice {
                    who: crate::ir::action::Who::You,
                    from: ZoneKindSel::Hand,
                    to: ZoneKindSel::Exile,
                    verb: MoveVerb::Exile,
                    filter: pitch_blue_filter(),
                    count: Expr::Num(1),
                    bind_as: Some("$fow_pitch"),
                },
                Action::PayLife {
                    who: crate::ir::action::Who::You,
                    amount: Expr::Num(1),
                },
            ])),
            hand_min: 2,
            ..Default::default()
        },
    ];
    c
}

/// Add {B}{B}{B}. CR 106.3.
fn dark_ritual() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, ManaSpec, Who as IrWho};
    use crate::ir::expr::Expr;
    let mut card = simple("Dark Ritual", CardKind::Instant(SpellData {
        mana_cost: "B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("B", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::AddMana {
                    who: IrWho::You,
                    count: Expr::Num(3),
                    spec: ManaSpec::Fixed(vec![Color::Black, Color::Black, Color::Black]),
                },
            }],
        },
        text: Some("Add {B}{B}{B}."),
    }];
    card
}

/// Destroy target creature with MV ≤ 3. CR 701.7.
fn fatal_push() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let mut card = simple("Fatal Push", CardKind::Instant(SpellData {
        mana_cost: "B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("B", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Battlefield,
                    filter: ir_and(ir_type(CardType::Creature), ir_mv_le(3)),
                },
                body: Action::Destroy { target: Expr::Ctx(Ctx::Var("target")) },
            }],
        },
        text: Some("Destroy target creature with mana value 3 or less."),
    }];
    card
}

/// Destroy target non-black creature. Alternate cost: pay 4 life (free spell). CR 701.7.
fn snuff_out() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let mut c = simple("Snuff Out", CardKind::Instant(SpellData {
        mana_cost: "3BB".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("3BB", false, true), None);
    c.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Battlefield,
                    filter: ir_and(ir_type(CardType::Creature), ir_not(ir_color(Color::Black))),
                },
                body: Action::Destroy { target: Expr::Ctx(Ctx::Var("target")) },
            }],
        },
        text: Some("Destroy target non-black creature."),
    }];
    // Phase 4 step 5 follow-up: the simplest IR alt cost — just PayLife(4).
    // No object decision, no schema entries; the executor drains life
    // directly and `cost_exec::pay` returns an empty CostsPaidCtx.
    c.alternate_costs = vec![
        AlternateCost {
            costs: CostBody::Ir(Action::PayLife {
                who: crate::ir::action::Who::You,
                amount: Expr::Num(4),
            }),
            ..Default::default()
        },
    ];
    c
}

/// Exile target creature. Its controller gains life equal to its power. CR 701.10.
fn swords_to_plowshares() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let mut card = simple("Swords to Plowshares", CardKind::Instant(SpellData {
        mana_cost: "W".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("W", true, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Battlefield,
                    filter: ir_type(CardType::Creature),
                },
                // Gain life = power FIRST (read while the creature is still on the
                // battlefield), then exile it. Same outcome as the closure.
                body: Action::Sequence(vec![
                    Action::GainLife {
                        who: IrWho::Player(Expr::Controller(Box::new(Expr::Ctx(Ctx::Var("target"))))),
                        amount: Expr::Power(Box::new(Expr::Ctx(Ctx::Var("target")))),
                    },
                    Action::Exile { target: Expr::Ctx(Ctx::Var("target")), bind_as: None },
                ]),
            }],
        },
        text: Some("Exile target creature. Its controller gains life equal to its power."),
    }];
    card
}

/// Destroy target creature or planeswalker.
/// Additional cost: discard a card OR pay 3 life (CR 118.9d).
fn bitter_triumph() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::context::Ctx;
    let mut def = simple("Bitter Triumph", CardKind::Instant(SpellData {
        mana_cost: "1B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("1B", false, false), None);
    def.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Battlefield,
                    filter: ir_or(ir_type(CardType::Creature), ir_type(CardType::Planeswalker)),
                },
                body: crate::ir::action::Action::Destroy {
                    target: crate::ir::expr::Expr::Ctx(Ctx::Var("target")),
                },
            }],
        },
        text: Some("Destroy target creature or planeswalker."),
    }];
    // "As an additional cost to cast this spell, discard a card or pay 3 life."
    // A cost-tree Choose (CR 601.2b): the chooser commits a branch at
    // announcement; the discard branch carries a nested object decision
    // (which card), answered via the same BindEnv. `It != Source` excludes the
    // spell itself, which is still in hand during the pre-cast feasibility check.
    def.additional_costs = {
        use crate::ir::action::{Action, ChoiceOption, MoveVerb, Who};
        use crate::ir::context::Ctx;
        use crate::ir::expr::{Expr, Filter, ZoneKindSel};
        CostBody::Ir(Action::Choose {
            who: Who::You,
            prompt: "Bitter Triumph: additional cost",
            options: vec![
                ChoiceOption {
                    label: "discard a card",
                    cost: None,
                    action: Box::new(Action::MoveByChoice {
                        who: Who::You,
                        from: ZoneKindSel::Hand,
                        to: ZoneKindSel::Graveyard,
                        verb: MoveVerb::Discard,
                        filter: Filter(Expr::Not(Box::new(Expr::Eq(
                            Box::new(Expr::Ctx(Ctx::It)),
                            Box::new(Expr::Ctx(Ctx::Source)),
                        )))),
                        count: Expr::Num(1),
                        bind_as: Some("$bt_discard"),
                    }),
                },
                ChoiceOption {
                    label: "pay 3 life",
                    cost: None,
                    action: Box::new(Action::PayLife { who: Who::You, amount: Expr::Num(3) }),
                },
            ],
            bind_as: Some("$bt_branch"),
        })
    };
    def
}

/// Destroy target creature or planeswalker with MV ≤ 3. This spell can't be countered (CR 608.2b).
fn long_goodbye() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let mut card = CardDef::new(
        "Long Goodbye",
        CardKind::Instant(SpellData {
            mana_cost: "1B".to_string(),
            modes: None,
            ..Default::default()
        }),
        parse_colors("1B", false, false),
        None,
        vec![], // supertypes
        CardLayout::Normal, None,
        vec![], vec![],
        vec![], // "can't be countered" is now an IR Prohibition (card.abilities below)
        vec![],
    );
    card.abilities = vec![
        Ability {
            kind: AbilityKind::OnResolve {
                modes: vec![IrSpellMode {
                    target_spec: TargetSpec::ObjectInZone {
                        controller: Who::Opp,
                        zone: ZoneId::Battlefield,
                        filter: ir_and(
                            ir_or(ir_type(CardType::Creature), ir_type(CardType::Planeswalker)),
                            ir_mv_le(3),
                        ),
                    },
                    body: Action::Destroy { target: Expr::Ctx(Ctx::Var("target")) },
                }],
            },
            text: Some("Destroy target creature or planeswalker with mana value 3 or less."),
        },
        // "This spell can't be countered" — IR Prohibition (event pipeline).
        cant_be_countered_self(),
    ];
    card
}

/// Choose one — each opponent sacrifices a nontoken creature (mode 0), a creature token
/// (mode 1), or a planeswalker (mode 2) of their choice. CR 700.2, CR 701.16.
/// Mode chosen at cast time (CR 700.2a); sacrifice goes through `sacrifice_choice`.
fn sheoldreds_edict() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::Expr;
    // Each opponent sacrifices the chosen kind of permanent. Modeled as
    // `Action::Sacrifice { who: Opponent }` — the sacrificing player picks via
    // `Strategy::sacrifice_choice`, same as the old `eff_sacrifice`.
    let sac = |filter| IrSpellMode {
        target_spec: TargetSpec::None,
        body: Action::Sacrifice {
            who: IrWho::Opponent,
            filter,
            count: Expr::Num(1),
            bind_as: None,
        },
    };
    let mut card = simple("Sheoldred's Edict", CardKind::Instant(SpellData {
        mana_cost: "1B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("1B", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![
                sac(ir_and(ir_not(ir_token()), ir_type(CardType::Creature))),
                sac(ir_token()),
                sac(ir_type(CardType::Planeswalker)),
            ],
        },
        text: Some("Choose one — each opponent sacrifices a creature; or a token; or a planeswalker."),
    }];
    card
}

/// Counter target noncreature spell unless its controller pays {2}. CR 700.2.
fn spell_pierce() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    let mut card = simple("Spell Pierce", CardKind::Instant(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", true, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Stack,
                    filter: ir_and(ir_spell(), ir_not(ir_type(CardType::Creature))),
                },
                body: counter_unless_pays_body(parse_mana_cost("2")),
            }],
        },
        text: Some("Counter target noncreature spell unless its controller pays {2}."),
    }];
    card
}

/// Counter target activated or triggered ability. (Mana abilities can't be targeted.)
/// Mana abilities never go on the stack (CR 605.3a), so `ir_ability()` already excludes them.
fn stifle() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;

    let mut card = simple("Stifle", CardKind::Instant(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", true, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Stack,
                    filter: ir_ability(),
                },
                body: Action::Counter { target: Expr::Ctx(Ctx::Var("target")) },
            }],
        },
        text: Some("Counter target activated or triggered ability."),
    }];
    card
}

/// Counter target instant or sorcery spell unless its controller pays {1}.
/// Storm (CR 702.40): when you cast this spell, copy it for each spell cast before it
/// this turn. Copies are counterable stack abilities targeting other legal targets.
///
/// IR structure (no `CounterUnlessPays` primitive):
/// - OnResolve: `Choose { who: Controller(target), pay {1} → Noop | else → Counter }`.
///   Payment-costed Choose options subsume the "unless X pays Y" idiom (CR 118.4
///   filters out unpayable options before the chooser sees them).
/// - Storm trigger (Triggered, active_zone: Stack): condition checks self-cast,
///   body copies the spell N-1 times where N = EventCount(ThisTurn, SpellCast
///   by controller). -1 excludes the triggering Flusterstorm cast itself (the
///   event log pushes *before* triggers fire).
fn flusterstorm() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, EventPattern, IrSpellMode, TriggerSpec};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::event_log::Window;
    use crate::ir::expr::{EventFilter, Expr, Filter, ZoneKindSel};

    let target_spec = TargetSpec::ObjectInZone {
        controller: Who::Opp,
        zone: ZoneId::Stack,
        filter: ir_or(
            ir_type(CardType::Instant),
            ir_type(CardType::Sorcery),
        ),
    };

    // "Counter unless its controller pays {1}" — shared with Daze / Spell Pierce.
    let on_resolve_body = counter_unless_pays_body(parse_mana_cost("1"));

    // Self-cast detection: the SpellCast pattern binds `triggered_obj` to the
    // cast card_id. Self-trigger ⇔ triggered_obj == Ctx::Source. Also require
    // caster == controller (defensive; storm is a "when you cast" trigger).
    let self_cast = Expr::And(
        Box::new(Expr::Eq(
            Box::new(Expr::Ctx(Ctx::Var("triggered_obj"))),
            Box::new(Expr::Ctx(Ctx::Source)),
        )),
        Box::new(Expr::Eq(
            Box::new(Expr::Ctx(Ctx::Var("triggered_actor"))),
            Box::new(Expr::Ctx(Ctx::Controller)),
        )),
    );

    // N = |SpellCast events this turn by controller| - 1.
    // The -1 excludes the Flusterstorm cast itself (already logged by fire_event).
    let storm_count = Expr::Sub(
        Box::new(Expr::EventCount {
            window: Window::ThisTurn,
            filter: Box::new(EventFilter::SpellCast {
                caster: Some(Box::new(Expr::Ctx(Ctx::Controller))),
            }),
        }),
        Box::new(Expr::Num(1)),
    );

    let storm_body = Action::CopySpell {
        what: Expr::Ctx(Ctx::Source),
        n: storm_count,
        new_targets: true,
    };

    let spell_data = SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    };
    let mut card = simple(
        "Flusterstorm",
        CardKind::Instant(spell_data),
        parse_colors("U", true, false),
        None,
    );
    card.abilities = vec![
        Ability {
            kind: AbilityKind::OnResolve {
                modes: vec![IrSpellMode {
                    target_spec,
                    body: on_resolve_body,
                }],
            },
            text: Some("Counter target instant or sorcery spell unless its controller pays {1}."),
        },
        Ability {
            kind: AbilityKind::Triggered {
                spec: TriggerSpec::When {
                    pattern: EventPattern::SpellCast {
                        spell_filter: Filter(Expr::Bool(true)),
                    },
                    condition: Some(self_cast),
                },
                target_spec: TargetSpec::None,
                body: storm_body,
                active_zone: ZoneKindSel::Stack,
            },
            text: Some("Storm (When you cast this spell, copy it for each spell cast before it this turn. You may choose new targets for the copies.)"),
        },
    ];
    card
}

/// Exile any number of target spells. If an opponent cast three or more spells this turn,
/// you may pay {0} rather than pay this spell's mana cost. CR 107.1c, CR 118.9.
fn mindbreak_trap() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};

    let mut c = simple("Mindbreak Trap", CardKind::Instant(SpellData {
        mana_cost: "2UU".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("2UU", true, false), None);
    // "Exile any number of target spells" — a generic object filter over the
    // stack: exile each spell not controlled by the caster. (No strategic reason
    // to spare any, so the "any number" choice collapses to "all opposing spells".)
    let not_mine = Filter(Expr::Not(Box::new(Expr::Eq(
        Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::It)))),
        Box::new(Expr::Ctx(Ctx::Controller)),
    ))));
    c.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: ir_for_each_obj(
                    ZoneKindSel::Stack,
                    ir_and(ir_spell(), not_mine),
                    Action::Exile { target: Expr::Ctx(Ctx::Var("v")), bind_as: None },
                ),
            }],
        },
        text: Some("Exile each spell an opponent controls. (Mindbreak Trap: exile any number of target spells.)"),
    }];
    c.alternate_costs = vec![
        AlternateCost {
            // Mindbreak Trap's alt cost is "{0}" — pay nothing — when the
            // condition is met. IR Action::Noop.
            costs: CostBody::Ir(crate::ir::action::Action::Noop),
            condition: Some(Arc::new(|caster, state| {
                state.player(caster.opp()).spells_cast_this_turn >= 3
            })),
            ..Default::default()
        },
    ];
    c
}

/// Counter target triggered ability or colorless spell.
/// Replicate {1} (CR 702.58): optional additional cost paid 0+ times; each payment
/// creates a copy of the spell targeting another triggered ability or colorless spell.
fn consign_to_memory() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let mut def = simple("Consign to Memory", CardKind::Instant(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);
    def.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Stack,
                    filter: ir_or(
                        ir_triggered_ability(),
                        ir_and(ir_spell(), ir_colorless()),
                    ),
                },
                body: Action::Counter { target: Expr::Ctx(Ctx::Var("target")) },
            }],
        },
        text: Some("Counter target triggered ability or colorless spell."),
    }];
    def.additional_costs = CostBody::Ir(Action::Replicate(parse_mana_cost("1")));
    def
}

/// Exile target card in a graveyard (not basic land), then exile all cards with the
/// same name from that player's graveyard, hand, and library (CR 107.4f phyrexian mana).
/// {B/P}: pay {B} or pay 2 life.
fn surgical_extraction() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel, ZoneSel};
    let mut c = simple("Surgical Extraction", CardKind::Instant(SpellData {
        mana_cost: "B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("B", false, false), None);
    // Exile the target plus every same-name card from its owner's graveyard,
    // hand, and library. One sweep per zone: for each object in that zone owned
    // by the target's owner whose name equals the target's name, exile it. The
    // target itself lives in the graveyard, so the graveyard sweep catches it.
    let sweep = |zone_kind: ZoneKindSel| Action::ForEach {
        over: Expr::AllObjects {
            zone: ZoneSel::Scoped {
                zone_kind,
                owner: Box::new(Expr::Owner(Box::new(Expr::Ctx(Ctx::Var("target"))))),
            },
            bind: "it",
            filter: Box::new(Expr::Eq(
                Box::new(Expr::Name(Box::new(Expr::Ctx(Ctx::Var("it"))))),
                Box::new(Expr::Name(Box::new(Expr::Ctx(Ctx::Var("target"))))),
            )),
        },
        bind: "v",
        body: Box::new(Action::Exile {
            target: Expr::Ctx(Ctx::Var("v")),
            bind_as: None,
        }),
    };
    c.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Graveyard,
                    filter: ir_not(ir_and(
                        ir_type(CardType::Land),
                        ir_supertype(Supertype::Basic),
                    )),
                },
                body: Action::Sequence(vec![
                    sweep(ZoneKindSel::Graveyard),
                    sweep(ZoneKindSel::Hand),
                    sweep(ZoneKindSel::Library),
                ]),
            }],
        },
        text: Some("Choose target card in a graveyard other than a basic land card. Exile it and all cards with the same name from that player's graveyard, hand, and library."),
    }];
    c.alternate_costs = vec![
        AlternateCost { costs: ir_pay_life(2), ..Default::default() },
    ];
    c
}

/// Build a TargetSpec for the modal color-hate instants: either a spell on the stack
/// or a permanent on the battlefield, both filtered to the given color.
fn color_hate_target_spec(c: Color) -> TargetSpec {
    TargetSpec::Union(vec![
        TargetSpec::ObjectInZone {
            controller: Who::Opp,
            zone: ZoneId::Stack,
            filter: ir_color(c),
        },
        TargetSpec::ObjectInZone {
            controller: Who::Opp,
            zone: ZoneId::Battlefield,
            filter: ir_color(c),
        },
    ])
}

/// Build a TargetSpec for the "if it's [color]" variant: targets ANY spell on the stack
/// or ANY permanent on the battlefield (targeting is unrestricted; the effect is conditional).
fn any_spell_or_permanent_target() -> TargetSpec {
    TargetSpec::Union(vec![
        TargetSpec::ObjectInZone {
            controller: Who::Opp,
            zone: ZoneId::Stack,
            filter: ir_spell(),
        },
        TargetSpec::ObjectInZone {
            controller: Who::Opp,
            zone: ZoneId::Battlefield,
            filter: ir_any(),
        },
    ])
}

/// IR body: counter `target` if it's on the stack (a spell), otherwise destroy
/// it (CR 701.5/701.7). Used by REB/BEB, where the color restriction lives on the
/// target spec rather than the effect. `target` is bound as `Ctx::Var("target")`.
fn ir_counter_or_destroy() -> crate::ir::action::Action {
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let target = || Expr::Ctx(Ctx::Var("target"));
    Action::IfThen {
        cond: Expr::Eq(
            Box::new(Expr::ZoneOf(Box::new(target()))),
            Box::new(Expr::ZoneLit(ZoneId::Stack)),
        ),
        then: Box::new(Action::Counter { target: target() }),
        else_: Some(Box::new(Action::Destroy { target: target() })),
    }
}

/// IR body: counter-or-destroy `target`, but only if it's the given color.
/// Pyroblast/Hydroblast may target any spell/permanent; the effect applies only
/// "if it's [color]" (CR 608.2b — a legal target whose effect simply doesn't
/// apply otherwise). Colors are read materialized, so Painter's Servant naming a
/// color makes a once-off-color permanent a valid victim.
fn ir_counter_or_destroy_if_color(c: Color) -> crate::ir::action::Action {
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    Action::IfThen {
        cond: Expr::Contains(
            Box::new(Expr::ColorLit(c)),
            Box::new(Expr::Colors(Box::new(Expr::Ctx(Ctx::Var("target"))))),
        ),
        then: Box::new(ir_counter_or_destroy()),
        else_: None,
    }
}

/// Lightning Bolt deals 3 damage to any target. CR 120.2.
fn lightning_bolt() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;

    let mut card = simple("Lightning Bolt", CardKind::Instant(SpellData {
        mana_cost: "R".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("R", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::Union(vec![
                    TargetSpec::ObjectInZone { controller: Who::Opp, zone: ZoneId::Battlefield, filter: ir_type(CardType::Creature) },
                    TargetSpec::ObjectInZone { controller: Who::Opp, zone: ZoneId::Battlefield, filter: ir_type(CardType::Planeswalker) },
                    TargetSpec::Player(Who::Opp),
                ]),
                body: Action::DealDamage {
                    source: Expr::Ctx(Ctx::Source),
                    target: Expr::Ctx(Ctx::Var("target")),
                    amount: Expr::Num(3),
                },
            }],
        },
        text: Some("Lightning Bolt deals 3 damage to any target."),
    }];
    card
}

/// Choose one — Deal 3 damage to target creature; or destroy target artifact. CR 700.2, 701.7.
fn abrade() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;

    let mut card = simple("Abrade", CardKind::Instant(SpellData {
        mana_cost: "1R".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("1R", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![
                // Mode 0: deal 3 damage to target creature
                IrSpellMode {
                    target_spec: TargetSpec::ObjectInZone {
                        controller: Who::Opp,
                        zone: ZoneId::Battlefield,
                        filter: ir_type(CardType::Creature),
                    },
                    body: Action::DealDamage {
                        source: Expr::Ctx(Ctx::Source),
                        target: Expr::Ctx(Ctx::Var("target")),
                        amount: Expr::Num(3),
                    },
                },
                // Mode 1: destroy target artifact
                IrSpellMode {
                    target_spec: TargetSpec::ObjectInZone {
                        controller: Who::Opp,
                        zone: ZoneId::Battlefield,
                        filter: ir_type(CardType::Artifact),
                    },
                    body: Action::Destroy { target: Expr::Ctx(Ctx::Var("target")) },
                },
            ],
        },
        text: Some("Choose one — Abrade deals 3 damage to target creature; or destroy target artifact."),
    }];
    card
}

/// Helper: an `Instant` whose only resolution body is `body`, targeting `target_spec`.
fn ir_instant(name: &str, mana_cost: &str, target_spec: TargetSpec,
              body: crate::ir::action::Action, text: &'static str) -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    let mut card = simple(name, CardKind::Instant(SpellData {
        mana_cost: mana_cost.to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors(mana_cost, false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve { modes: vec![IrSpellMode { target_spec, body }] },
        text: Some(text),
    }];
    card
}

/// Counter target blue spell, or destroy target blue permanent. CR 701.5, 701.7.
fn red_elemental_blast() -> CardDef {
    ir_instant("Red Elemental Blast", "R", color_hate_target_spec(Color::Blue),
        ir_counter_or_destroy(),
        "Counter target blue spell, or destroy target blue permanent.")
}

/// Counter target spell if it's blue; or destroy target permanent if it's blue.
/// Targets any opp spell/permanent; effect fizzles if the target is not blue. CR 701.5, 701.7.
fn pyroblast() -> CardDef {
    ir_instant("Pyroblast", "R", any_spell_or_permanent_target(),
        ir_counter_or_destroy_if_color(Color::Blue),
        "Choose one — Counter target spell if it's blue; or destroy target permanent if it's blue.")
}

/// Counter target red spell, or destroy target red permanent. CR 701.5, 701.7.
fn blue_elemental_blast() -> CardDef {
    ir_instant("Blue Elemental Blast", "U", color_hate_target_spec(Color::Red),
        ir_counter_or_destroy(),
        "Counter target red spell, or destroy target red permanent.")
}

/// Counter target spell if it's red; or destroy target permanent if it's red.
/// Targets any opp spell/permanent; effect fizzles if the target is not red. CR 701.5, 701.7.
fn hydroblast() -> CardDef {
    ir_instant("Hydroblast", "U", any_spell_or_permanent_target(),
        ir_counter_or_destroy_if_color(Color::Red),
        "Choose one — Counter target spell if it's red; or destroy target permanent if it's red.")
}

// ── Sorceries ─────────────────────────────────────────────────────────────────

/// All creatures get -X/-X until end of turn; additional cost: pay X life (CR 107.2).
/// The -X/-X is a Layer 7 ContinuousInstance; creatures with resulting toughness ≤ 0
/// die when the engine checks state-based actions before the next priority grant.
/// X is chosen by the strategy (default: 3) via `choose_x_for_spell`.
fn toxic_deluge() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Expiry as IrExpiry};
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel, ZoneSel};
    let mut def = simple("Toxic Deluge", CardKind::Sorcery(SpellData {
        mana_cost: "2B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("2B", false, false), None);
    // -X/-X to every creature until end of turn: one L7 P/T continuous effect over
    // the creatures present at resolution (`ApplyCE` over the `AllObjects` set →
    // a single CI; layer derived from the `PumpPT` write). X = announced X life
    // paid, read as `Ctx::Var("x")` and negated.
    let neg_x = Expr::Neg(Box::new(Expr::Ctx(Ctx::Var("x"))));
    def.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::ApplyCE {
                    target: Expr::AllObjects {
                        zone: ZoneSel::Global(ZoneKindSel::Battlefield),
                        bind: "it",
                        filter: Box::new(Expr::Contains(
                            Box::new(Expr::TypeLit(CardType::Creature)),
                            Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::Var("it"))))),
                        )),
                    },
                    mods: vec![CEMod::PumpPT(neg_x.clone(), neg_x)],
                    expiry: IrExpiry::EndOfTurn,
                },
            }],
        },
        text: Some("All creatures get -X/-X until end of turn, where X is the amount of life paid."),
    }];
    def.additional_costs = ir_xlife_cost();
    def
}

/// "For each object in `zone` (across all players) matching `filter`, run `body`."
/// `body` refers to the current match as `Ctx::Var("v")`. The generic
/// object-set sweep: a board wipe is this over `Battlefield`, Mindbreak Trap is
/// this over `Stack`. Protection/indestructibility live in the leaf primitives
/// (`DealDamage` / `Destroy`), so this stays a pure iteration.
fn ir_for_each_obj(zone: crate::ir::expr::ZoneKindSel, filter: crate::ir::expr::Filter,
                   body: crate::ir::action::Action) -> crate::ir::action::Action {
    use crate::ir::action::Action;
    use crate::ir::expr::{Expr, ZoneSel};
    Action::ForEach {
        over: Expr::AllObjects {
            zone: ZoneSel::Global(zone),
            bind: "it",
            filter: Box::new(filter.0),
        },
        bind: "v",
        body: Box::new(body),
    }
}

/// `ir_for_each_obj` specialized to the battlefield — the common board-wide sweep.
fn ir_for_each_on_battlefield(filter: crate::ir::expr::Filter, body: crate::ir::action::Action) -> crate::ir::action::Action {
    ir_for_each_obj(crate::ir::expr::ZoneKindSel::Battlefield, filter, body)
}

/// Brotherhood's End — {1}{R}{R} sorcery. Choose one:
/// • Deal 3 damage to each creature and each planeswalker.
/// • Destroy all artifacts with mana value 3 or less.
fn brotherhoods_end() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;

    let mut card = simple("Brotherhood's End", CardKind::Sorcery(SpellData {
        mana_cost: "1RR".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("1RR", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![
                // Mode 0: 3 damage to each creature and each planeswalker.
                IrSpellMode {
                    target_spec: TargetSpec::None,
                    body: ir_for_each_on_battlefield(
                        ir_or(ir_type(CardType::Creature), ir_type(CardType::Planeswalker)),
                        Action::DealDamage {
                            source: Expr::Ctx(Ctx::Source),
                            target: Expr::Ctx(Ctx::Var("v")),
                            amount: Expr::Num(3),
                        },
                    ),
                },
                // Mode 1: destroy all artifacts with mana value 3 or less.
                IrSpellMode {
                    target_spec: TargetSpec::None,
                    body: ir_for_each_on_battlefield(
                        ir_and(ir_type(CardType::Artifact), ir_mv_le(3)),
                        Action::Destroy { target: Expr::Ctx(Ctx::Var("v")) },
                    ),
                },
            ],
        },
        text: Some("Choose one — 3 damage to each creature and planeswalker; or destroy each artifact with mana value 3 or less."),
    }];
    card
}

/// Win condition: set success=true. In full rules: opponent's library and graveyard become
/// their library; controller searches for exactly five cards. CR 101.1 (shortcut).
fn doomsday() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    // "Doomsday" is a sentinel in this sim: resolving it is the stopping point
    // (the real pile-building is deferred to the human via the web UI). The card
    // body is a deliberate no-op — termination + life accounting are owned by the
    // application's objective (`objective::DoomsdayResolvedObjective`), which
    // observes the `SpellResolved` event. Not a real cast.
    let mut card = simple("Doomsday", CardKind::Sorcery(SpellData {
        mana_cost: "BBB".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("BBB", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Sequence(vec![]),
            }],
        },
        text: Some("(sim sentinel) Resolving Doomsday ends the simulation; the objective observes it."),
    }];
    card
}

/// Look at top 5, put two in hand, rest on bottom in any order. Modeled as draw:2. CR 701.26.
fn stock_up() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::Expr;
    let mut card = simple("Stock Up", CardKind::Sorcery(SpellData {
        mana_cost: "2U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                // De-hacked: real "look at top 5, keep 2, rest to bottom" — a dig,
                // not a Draw N (which was a player-agency + too-specific shortcut).
                body: Action::Dig { who: IrWho::You, n: Expr::Num(5), take: Expr::Num(2) },
            }],
        },
        text: Some("Look at the top five cards of your library. Put two of them into your hand and the rest on the bottom of your library in any order."),
    }];
    card
}

/// Look at the top three cards; put one (two if there's an instant AND a sorcery
/// in your graveyard) into hand, rest on the bottom. CR 701.* (look/choose).
fn flow_state() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel, ZoneSel};
    let mut card = simple("Flow State", CardKind::Sorcery(SpellData {
        mana_cost: "1U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);

    // ">= 1 card of `ty` in your graveyard".
    let gy_has = |ty: CardType, bind: &'static str| -> Expr {
        Expr::Ge(
            Box::new(Expr::Count(Box::new(Expr::AllObjects {
                zone: ZoneSel::Scoped {
                    zone_kind: ZoneKindSel::Graveyard,
                    owner: Box::new(Expr::Ctx(Ctx::Controller)),
                },
                bind,
                filter: Box::new(Expr::Contains(
                    Box::new(Expr::TypeLit(ty)),
                    Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::Var(bind))))),
                )),
            }))),
            Box::new(Expr::Num(1)),
        )
    };
    let cond = Expr::And(
        Box::new(gy_has(CardType::Instant, "fs_i")),
        Box::new(gy_has(CardType::Sorcery, "fs_s")),
    );

    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::IfThen {
                    cond,
                    then: Box::new(Action::Dig { who: IrWho::You, n: Expr::Num(3), take: Expr::Num(2) }),
                    else_: Some(Box::new(Action::Dig { who: IrWho::You, n: Expr::Num(3), take: Expr::Num(1) })),
                },
            }],
        },
        text: Some("Look at the top three cards of your library. Put one of them into your hand and the rest on the bottom of your library in any order. If there is an instant card and a sorcery card in your graveyard, instead put two of them into your hand and the rest on the bottom of your library in any order."),
    }];
    card
}

/// Scry 2, then draw a card. CR 701.18 (scry), CR 701.9 (draw).
fn preordain() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::Expr;
    let mut card = simple("Preordain", CardKind::Sorcery(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Sequence(vec![
                    Action::Scry { who: IrWho::You, n: Expr::Num(2) },
                    Action::Draw { who: IrWho::You, n: Expr::Num(1) },
                ]),
            }],
        },
        text: Some("Scry 2, then draw a card."),
    }];
    card
}

/// Look at top 3, arrange or shuffle, then draw. CR 701.26 (library manipulation).
/// "You may shuffle" decomposes to `MayDo { Shuffle }` — the shuffle is the
/// effect, the "may" is a y/n strategy decision (no heuristic baked in).
fn ponder() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::Expr;
    let mut card = simple("Ponder", CardKind::Sorcery(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                // Look at top 3, put back in any order (a player decision via
                // OrderTop → Strategy::order_top_library); you may shuffle; draw.
                body: Action::Sequence(vec![
                    Action::OrderTop { who: IrWho::You, n: Expr::Num(3) },
                    Action::MayDo {
                        who: IrWho::You,
                        action: Box::new(Action::Shuffle { who: IrWho::You }),
                    },
                    Action::Draw { who: IrWho::You, n: Expr::Num(1) },
                ]),
            }],
        },
        text: Some("Look at the top three cards of your library, then put them back in any order. You may shuffle. Draw a card."),
    }];
    card
}

/// Target opponent discards a nonland card; you lose 2 life. CR 701.8, CR 702.1.
fn thoughtseize() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel, ZoneSel};
    let mut card = simple("Thoughtseize", CardKind::Sorcery(SpellData {
        mana_cost: "B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("B", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Sequence(vec![
                    // Reveal the opponent's hand (every hand card not owned by us).
                    Action::Reveal {
                        who: IrWho::Opponent,
                        what: Expr::AllObjects {
                            zone: ZoneSel::Global(ZoneKindSel::Hand),
                            bind: "h",
                            filter: Box::new(Expr::Not(Box::new(Expr::Eq(
                                Box::new(Expr::Owner(Box::new(Expr::Ctx(Ctx::Var("h"))))),
                                Box::new(Expr::Ctx(Ctx::Controller)),
                            )))),
                        },
                    },
                    // Opponent discards a nonland card. (Random pick, as before.)
                    Action::Discard {
                        who: IrWho::Opponent,
                        count: Expr::Num(1),
                        at_random: true,
                        filter: Some(ir_not(ir_type(CardType::Land))),
                    },
                    // You lose 2 life.
                    Action::PayLife { who: IrWho::You, amount: Expr::Num(2) },
                ]),
            }],
        },
        text: Some("Target player reveals their hand. You choose a nonland card from it. That player discards that card. You lose 2 life."),
    }];
    card
}

/// Return target creature from your graveyard to play. CR 701.14.
/// Reanimation modeled with the generic `Move` (graveyard → battlefield), not a
/// dedicated reanimate primitive.
fn unearth() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut card = simple("Unearth", CardKind::Sorcery(SpellData {
        mana_cost: "B".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("B", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Actor,
                    zone: ZoneId::Graveyard,
                    filter: ir_type(CardType::Creature),
                },
                body: Action::Move {
                    what: Expr::Ctx(Ctx::Var("target")),
                    to: ZoneKindSel::Battlefield,
                    to_owner: None,
                    bind_as: None,
                },
            }],
        },
        text: Some("Return target creature card from your graveyard to the battlefield."),
    }];
    card
}

/// Target opponent discards 2 cards at random. CR 701.8.
fn hymn_to_tourach() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::Expr;
    let mut card = simple("Hymn to Tourach", CardKind::Sorcery(SpellData {
        mana_cost: "BB".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("BB", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Discard {
                    who: IrWho::Opponent,
                    count: Expr::Num(2),
                    at_random: true,
                    filter: None,
                },
            }],
        },
        text: Some("Target player discards two cards at random."),
    }];
    card
}

/// Cycling: discard this card, sacrifice a land you control → draw a card.
/// Modeled as a hand-zone activated ability. Cast cost {G}{W} rarely used.
fn edge_of_autumn() -> CardDef {
    simple("Edge of Autumn", CardKind::Sorcery(SpellData {
        mana_cost: "GW".to_string(),
        // Hand ability: discard self + sacrifice a land → draw 1.
        // Modeled via AbilityDef on SpellData is not standard; the TOML used `abilities`
        // at the top level. Since SpellData has no abilities field, this card has no
        // castable effects but the hand ability is registered via the cycling-like path
        // in strategy. Future work: add `abilities` to SpellData.
        ..Default::default()
    }), parse_colors("GW", false, false), None)
}

/// Search your library for a sorcery card, put it on top. CR 700.3, CR 701.19.
fn personal_tutor() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut card = simple("Personal Tutor", CardKind::Sorcery(SpellData {
        mana_cost: "U".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("U", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Search {
                    who: IrWho::You,
                    zone: ZoneKindSel::Library,
                    filter: ir_type(CardType::Sorcery),
                    count: Expr::Num(1),
                    dest: ZoneKindSel::Library,
                    to_top: true,
                    shuffle: true,
                    bind_as: None,
                },
            }],
        },
        text: Some("Search your library for a sorcery card and reveal that card. Shuffle your library, then put the card on top of it."),
    }];
    card
}

/// Search your library for a green creature and put it onto the battlefield.
/// X not modeled; treated as {1G} (fixed cost). CR 700.3, CR 701.19.
fn green_suns_zenith() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut card = simple("Green Sun's Zenith", CardKind::Sorcery(SpellData {
        mana_cost: "1G".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("1G", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: Action::Search {
                    who: IrWho::You,
                    zone: ZoneKindSel::Library,
                    filter: ir_and(ir_color(Color::Green), ir_type(CardType::Creature)),
                    count: Expr::Num(1),
                    dest: ZoneKindSel::Battlefield,
                    to_top: false,
                    shuffle: true,
                    bind_as: None,
                },
            }],
        },
        text: Some("Search your library for a green creature card, put it onto the battlefield, then shuffle."),
    }];
    card
}

/// Each player may put an artifact, creature, enchantment, or land card from their
/// hand onto the battlefield. Both placements are simultaneous (CR 101.4).
fn show_and_tell() -> CardDef {
    simple("Show and Tell", CardKind::Sorcery(SpellData {
        mana_cost: "2U".to_string(),
        modes: untargeted_mode(|who, _source_id, _x| {
            eff_each_may_put(who, ir_or(
                ir_or(ir_type(CardType::Artifact), ir_type(CardType::Creature)),
                ir_or(ir_type(CardType::Enchantment), ir_type(CardType::Land)),
            ))
        }),
        ..Default::default()
    }), parse_colors("U", false, false), None)
}

fn omniscience() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::ce::{CEMod, CostSpec};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};

    let mut card = CardDef::new(
        "Omniscience",
        CardKind::Enchantment(EnchantmentData::default()),
        parse_colors("UUUUUUUUU", false, false),  // blue; {7}{U}{U}{U}
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![], vec![],
        vec![], // effect is an IR Static ability (below)
    );
    // "You may cast spells from your hand without paying their mana costs."
    // An L3 alt-cost CE: push a free `AlternateCost` onto each non-land card the
    // Omniscience controller controls. Scope is controller-keyed but *zone-agnostic*
    // on purpose — a spell moves hand→stack at CR 601.2a, before its cost is paid, so
    // a hand-only scope would drop the alt cost mid-cast. (Only hand casts consume it
    // in practice.) `AltCost(Free)` is translated by `cemod_to_modifier`.
    let non_land_of_controller = Filter(Expr::And(
        Box::new(Expr::Eq(
            Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::It)))),
            Box::new(Expr::Ctx(Ctx::Controller)),
        )),
        Box::new(Expr::Not(Box::new(Expr::Contains(
            Box::new(Expr::TypeLit(CardType::Land)),
            Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::It)))),
        )))),
    ));
    card.abilities = vec![Ability {
        kind: AbilityKind::Static {
            mods: vec![CEMod::AltCost(CostSpec::Free)],
            scope: Some(non_land_of_controller),
            condition: None,
        },
        text: Some("You may cast spells from your hand without paying their mana costs."),
    }];
    card
}

fn sneak_attack() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, StepScope, TriggerSpec};
    use crate::ir::action::{Action, Expiry as IrExpiry, Who as IrWho};
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};

    // The creature picked from hand (CR 701.10 resolution choice) is bound as
    // `Ctx::Var("target")` by `build_ability_effect`.
    let chosen = || Expr::Ctx(Ctx::Var("target"));

    // "{R}: You may put a creature card from your hand onto the battlefield. That
    //  creature gains haste. Sacrifice it at the beginning of the next end step."
    let body = Action::Sequence(vec![
        // Put the chosen creature onto the battlefield (full ETB pipeline).
        Action::Move {
            what: chosen(),
            to: ZoneKindSel::Battlefield,
            to_owner: None,
            bind_as: None,
        },
        // It gains haste. EndOfTurn duration suffices — it's sacrificed this turn,
        // and the printed creature has no haste once the effect ends.
        Action::ApplyCE {
            target: chosen(),
            mods: vec![CEMod::AddKeyword(Keyword::Haste)],
            expiry: IrExpiry::EndOfTurn,
        },
        // Delayed: at the beginning of the next end step, sacrifice it. The
        // `target` binding is captured at schedule time, so it resolves to this
        // creature when the trigger fires (matches nothing if it has since left).
        Action::ScheduleDelayedTrigger {
            fires: TriggerSpec::AtStep { step: StepKind::End, who: StepScope::EachPlayer },
            action: Box::new(Action::Sacrifice {
                who: IrWho::You,
                filter: Filter(Expr::Eq(Box::new(Expr::Ctx(Ctx::It)), Box::new(chosen()))),
                count: Expr::Num(1),
                bind_as: None,
            }),
        },
    ]);

    let mut card = CardDef::new(
        "Sneak Attack",
        CardKind::Enchantment(EnchantmentData::default()),
        parse_colors("3R", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![], vec![],
        vec![],
    );
    card.abilities = vec![Ability {
        kind: AbilityKind::Activated {
            cost: ir_pay_mana_str("R"),
            target_spec: TargetSpec::None,
            choice_spec: Some(ChoiceSpec {
                controller: Who::Actor, // effects::Who, via super::*
                zone: ZoneId::Hand,
                filter: ir_type(CardType::Creature),
            }),
            body,
            timing: ActivationTiming::Default, // instant speed
            activation_condition: None,
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("{R}: You may put a creature card from your hand onto the battlefield. That creature gains haste. Sacrifice the creature at the beginning of the next end step."),
    }];
    card
}

// ── Creatures ─────────────────────────────────────────────────────────────────

/// ETB: look at top X cards of your library, where X is the number of cards in it;
/// if you control more blue/black permanents than opponent, you win. Modeled as win-on-ETB
/// via strategy, not via trigger here (no ETB trigger — strategy checks for Oracle).
/// CR 702.15 (devotion), CR 104.3b.
fn thassas_oracle() -> CardDef {
    let data = CreatureData::new("UU", 1, 3);
    simple("Thassa's Oracle", CardKind::Creature(data), parse_colors("UU", false, false), Some(1))
}

/// Cycling (hand ability): discard this + pay 2 life → draw 1. CR 702.28.
fn street_wraith() -> CardDef {
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut data = CreatureData::new("3BB", 3, 4);
    data.abilities = vec![AbilityDef {
        source_zone: SourceZone::Hand,
        // `DiscardSelf` is "send the source itself to the graveyard" — no
        // candidate enumeration. Use `Action::Move` (deterministic) rather
        // than `MoveByChoice` (which would try to find the source in `Hand`
        // — but the activation pipeline pre-moves hand-source abilities to
        // Stack before paying costs, so the source isn't in Hand anymore).
        costs: ir_seq(vec![
            Action::Move {
                what: Expr::Ctx(Ctx::Source),
                to: ZoneKindSel::Graveyard,
                to_owner: None,
                bind_as: None,
            },
            act_pay_life(2),
        ]),
        ir_body: Some(Action::Draw { who: IrWho::You, n: Expr::Num(1) }),
        ..Default::default()
    }];
    simple("Street Wraith", CardKind::Creature(data), parse_colors("3BB", false, false), Some(1))
}

/// 0/1 for {2B}. No special abilities — just a beater.
fn barrowgoyf() -> CardDef {
    let mut data = CreatureData::new("2B", 0, 1);
    data.legendary = false;
    simple("Barrowgoyf", CardKind::Creature(data), parse_colors("2B", false, true), None)
}

/// Ninjutsu {1U}: swap in with an unblocked attacker. CR 702.49.
fn ingenious_infiltrator() -> CardDef {
    let mut data = CreatureData::new("1UB", 2, 1);
    data.abilities = vec![ninjutsu_ability("1U")];
    data.creature_subtypes = vec!["Ninja".into()];
    simple(
        "Ingenious Infiltrator",
        CardKind::Creature(data),
        parse_colors("1UB", true, true),
        None,
    )
}

/// Legendary Planeswalker — Kaito. Loyalty 4. Ninjutsu {1UB}.
/// +1: emblem "Ninjas you control get +1/+1."
/// 0: Surveil 2, draw per opponent who lost life this turn.
/// −2: Tap target creature, put 2 stun counters on it.
/// Static: during your turn, if loyalty > 0, he's a 3/4 Ninja creature with hexproof.
fn kaito_bane_of_nightmares() -> CardDef {
    CardDef::new(
        "Kaito, Bane of Nightmares",
        CardKind::Planeswalker(PlaneswalkerData {
            mana_cost: "2UB".into(),
            loyalty: 4,
            abilities: vec![
                // Ninjutsu from hand (not a loyalty ability).
                ninjutsu_ability("1UB"),
                // +1: emblem "Ninjas you control get +1/+1."
                AbilityDef {
                    costs: ir_loyalty(1),
                    ability_factory: Some(Arc::new(build_kaito_plus_one)),
                    timing: ActivationTiming::Sorcery,
                    ..Default::default()
                },
                // 0: Surveil 2, draw if opp lost life.
                AbilityDef {
                    costs: ir_loyalty(0),
                    ability_factory: Some(Arc::new(build_kaito_zero)),
                    timing: ActivationTiming::Sorcery,
                    ..Default::default()
                },
                // −2: Tap target creature + 2 stun counters.
                AbilityDef {
                    costs: ir_loyalty(-2),
                    ability_factory: Some(Arc::new(build_kaito_minus_two)),
                    target_spec: TargetSpec::ObjectInZone {
                        controller: Who::Opp,
                        zone: ZoneId::Battlefield,
                        filter: ir_type(CardType::Creature),
                    },
                    timing: ActivationTiming::Sorcery,
                    ..Default::default()
                },
            ],
        }),
        parse_colors("2UB", true, true),
        None,
        vec![Supertype::Legendary], CardLayout::Normal, None,
        vec![],
        vec![replacement_planeswalker_etb(4)],
        vec![],
        vec![kaito_animation_ce()],
    )
}

/// Static CE for Kaito: "During your turn, as long as Kaito has one or more loyalty
/// counters on him, he's a 3/4 Ninja creature and has hexproof."
/// Modeled as a self-targeting L4 CE that conditionally makes Kaito a creature.
fn kaito_animation_ce() -> StaticAbilityDef {
    Arc::new(move |source_id, controller| ContinuousInstance {
        source_id,
        controller,
        layer: ContinuousLayer::L4TypeEffects,
        reads: vec![],
        writes: vec![CeWrites::CardTypes, CeWrites::PowerToughness, CeWrites::Abilities],
        timestamp: 0,
        filter: Arc::new(move |id, _ctr, _state| id == source_id),
        modifier: Arc::new(move |def, state| {
            // Check conditions: controller's turn AND loyalty > 0.
            let is_my_turn = state.current_ap == state.player_id(controller);
            let has_loyalty = state.permanent_bf(source_id)
                .map_or(false, |bf| bf.loyalty > 0);
            if !is_my_turn || !has_loyalty { return; }
            // Add Creature type (Kaito is now a Planeswalker Creature).
            if !def.types.contains(&CardType::Creature) {
                def.types.push(CardType::Creature);
            }
            // Set to 3/4 Ninja with hexproof.
            match &mut def.kind {
                CardKind::Planeswalker(pw) => {
                    // Overlay creature data: 3/4 Ninja creature with hexproof.
                    // P/T is set directly since this is a type-setting effect, not a modifier.
                    let mut c = CreatureData::new(&pw.mana_cost, 3, 4);
                    c.legendary = true;
                    c.creature_subtypes = vec!["Ninja".into()];
                    c.keywords.insert(Keyword::Hexproof);
                    // Carry over abilities so loyalty abilities remain activatable.
                    c.abilities = pw.abilities.clone();
                    def.kind = CardKind::Creature(c);
                }
                CardKind::Creature(c) => {
                    // Already animated (e.g. multiple CEs); just ensure stats.
                    c.creature_subtypes = vec!["Ninja".into()];
                    c.keywords.insert(Keyword::Hexproof);
                }
                _ => {}
            }
        }),
        expiry: Expiry::WhileSourceOnBattlefield,
    })
}

/// ETB: search your library for a creature with toughness ≤ 2, put it into your hand.
/// CR 700.3, CR 701.19.
fn recruiter_of_the_guard() -> CardDef {
    CardDef::new(
        "Recruiter of the Guard",
        CardKind::Creature(CreatureData::new("2W", 1, 1)),
        parse_colors("2W", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![TriggerDef { check: Arc::new(recruiter_check), active_when: tp_on_battlefield() }],
        vec![],
        vec![],
        vec![],
    )
}

/// Stoneforge Mystic — {1}{W} Creature — Kor Artificer 1/2.
/// "When this creature enters, you may search your library for an Equipment card,
///  reveal it, put it into your hand, then shuffle."
/// "{1}{W}, {T}: You may put an Equipment card from your hand onto the battlefield."
fn stoneforge_mystic() -> CardDef {
    let mut data = CreatureData::new("1W", 1, 2);
    data.creature_subtypes = vec!["Kor".into(), "Artificer".into()];
    data.abilities = vec![AbilityDef {
        // {1}{W}, {T}: put an Equipment card from your hand onto the battlefield.
        costs: ir_seq(vec![act_pay_mana_str("1W"), act_tap_source()]),
        choice_spec: Some(ChoiceSpec {
            controller: Who::Actor,
            zone: ZoneId::Hand,
            filter: ir_subtype("Equipment"),
        }),
        ability_factory: Some(Arc::new(|who, _source_id| {
            Effect(Arc::new(move |state, t, targets| {
                let Some(&equip_id) = targets.first() else { return };
                let name = state.objects.get(&equip_id)
                    .map(|c| c.catalog_key.clone())
                    .unwrap_or_default();
                state.log(t, who, format!("Stoneforge Mystic → {} onto the battlefield", name));
                change_zone(equip_id, ZoneId::Battlefield, state, t, who);
            }))
        })),
        timing: ActivationTiming::Sorcery,
        ..Default::default()
    }];
    CardDef::new(
        "Stoneforge Mystic",
        CardKind::Creature(data),
        parse_colors("1W", false, false),
        None,
        vec![], CardLayout::Normal, None,
        // ETB trigger: search library for an Equipment card, put into hand.
        vec![TriggerDef {
            check: Arc::new(|event, source_id, controller, _state, pending| {
                if let GameEvent::ZoneChange { id, to: ZoneId::Battlefield, controller: ctlr, .. } = event {
                    if *id == source_id && *ctlr == controller {
                        pending.push(TriggerContext {
                            source_name: "Stoneforge Mystic".into(),
                            controller,
                            target_spec: TargetSpec::None,
                            effect: eff_fetch_search(controller, ir_subtype("Equipment"), ZoneId::Hand),
                        });
                    }
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![], vec![],
    )
}

/// ETB trigger + draw-trigger: deal 1 damage to any target and amass Orc 1 whenever
/// opponent draws a non-natural card. Also fires on its own ETB. CR 603.
fn orcish_bowmasters() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, EventPattern, TriggerSpec};
    use crate::ir::action::{Action, TokenSpec, Who as IrWho};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel, ZoneSel};

    let mut data = CreatureData::new("1B", 1, 1);
    data.legendary = false;
    let mut card = CardDef::new(
        "Orcish Bowmasters",
        CardKind::Creature(data),
        parse_colors("1B", false, true),
        None,
        vec![], CardLayout::Normal, None,
        vec![],
        vec![],
        vec![],
        vec![],
    );

    // Shared body: "deal 1 damage to any target; amass Orc 1".
    let any_target = TargetSpec::Union(vec![
        TargetSpec::ObjectInZone { controller: Who::Opp, zone: ZoneId::Battlefield, filter: ir_type(CardType::Creature) },
        TargetSpec::ObjectInZone { controller: Who::Opp, zone: ZoneId::Battlefield, filter: ir_type(CardType::Planeswalker) },
        TargetSpec::Player(Who::Opp),
    ]);
    // Amass Orcs 1 decomposed:
    //   if you control no Orc Army, first create a 0/0 Orc Army token.
    //   for each Orc Army you control, put a +1/+1 counter on it.
    // The re-query after CreateToken means the freshly-minted army is found
    // without an explicit bind. SBAs don't run mid-Sequence, so the 0/0
    // survives long enough to grow.
    let orc_army_set = || Expr::AllObjects {
        zone: ZoneSel::Global(ZoneKindSel::Battlefield),
        bind: "a",
        filter: Box::new(Expr::And(
            Box::new(Expr::Eq(
                Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::Var("a"))))),
                Box::new(Expr::Ctx(Ctx::Controller)),
            )),
            Box::new(Expr::Eq(
                Box::new(Expr::Name(Box::new(Expr::Ctx(Ctx::Var("a"))))),
                Box::new(Expr::NameLit("Orc Army".to_string())),
            )),
        )),
    };
    let amass_orcs_1 = Action::Sequence(vec![
        Action::IfThen {
            cond: Expr::Eq(
                Box::new(Expr::Count(Box::new(orc_army_set()))),
                Box::new(Expr::Num(0)),
            ),
            then: Box::new(Action::CreateToken {
                who: IrWho::You,
                spec: TokenSpec {
                    name: "Orc Army",
                    types: vec![CardType::Creature],
                    subtypes: vec![],
                    colors: vec![],
                    power: Some(0),
                    toughness: Some(0),
                    keywords: vec![],
                },
                n: Expr::Num(1),
            }),
            else_: None,
        },
        Action::ForEach {
            over: orc_army_set(),
            bind: "a",
            body: Box::new(Action::PutCounters {
                on: Expr::Ctx(Ctx::Var("a")),
                kind: CounterType::PlusOnePlusOne,
                n: Expr::Num(1),
            }),
        },
    ]);
    let body = Action::Sequence(vec![
        Action::DealDamage {
            source: Expr::Ctx(Ctx::Source),
            target: Expr::Ctx(Ctx::Var("target")),
            amount: Expr::Num(1),
        },
        amass_orcs_1,
    ]);

    // Filter: entering object == this Bowmasters (self-ETB).
    let self_etb = Filter(Expr::Eq(
        Box::new(Expr::Ctx(Ctx::It)),
        Box::new(Expr::Ctx(Ctx::Source)),
    ));
    // Filter: the drawing player is an opponent.
    let opp_draws = Filter(Expr::Not(Box::new(Expr::Eq(
        Box::new(Expr::Ctx(Ctx::It)),
        Box::new(Expr::Ctx(Ctx::Controller)),
    ))));
    // Condition: draw is not a natural draw-step draw.
    let not_natural = Expr::Not(Box::new(Expr::Ctx(Ctx::Var("triggered_is_natural"))));

    card.abilities = vec![
        Ability {
            kind: AbilityKind::Triggered {
                spec: TriggerSpec::When {
                    pattern: EventPattern::EntersZone {
                        obj_filter: self_etb,
                        zone_kind: ZoneKindSel::Battlefield,
                    },
                    condition: None,
                },
                target_spec: any_target.clone(),
                body: body.clone(),
                active_zone: ZoneKindSel::Battlefield,
            },
            text: Some("When Orcish Bowmasters enters, it deals 1 damage to any target. Amass Orcs 1."),
        },
        Ability {
            kind: AbilityKind::Triggered {
                spec: TriggerSpec::When {
                    pattern: EventPattern::Draw { who: opp_draws },
                    condition: Some(not_natural),
                },
                target_spec: any_target,
                body,
                active_zone: ZoneKindSel::Battlefield,
            },
            text: Some("Whenever an opponent draws a card except the first one they draw in each of their draw steps, Orcish Bowmasters deals 1 damage to any target. Amass Orcs 1."),
        },
    ];
    card
}

/// ETB replacement: enters with counters = # of instants/sorceries in controller's exile.
/// Trigger: gains +1/+1 counter when a spell is exiled from your graveyard. CR 614.1, CR 603.
fn murktide_regent() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, EventPattern, TriggerSpec};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};

    // Filter: triggering object is instant or sorcery.
    let is_inst_or_sorc = Filter(Expr::Or(
        Box::new(Expr::Contains(
            Box::new(Expr::TypeLit(CardType::Instant)),
            Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::It)))),
        )),
        Box::new(Expr::Contains(
            Box::new(Expr::TypeLit(CardType::Sorcery)),
            Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::It)))),
        )),
    ));
    // Actor filter: actor == controller (only when you exile).
    let actor_is_you = Filter(Expr::Eq(
        Box::new(Expr::Ctx(Ctx::It)),
        Box::new(Expr::Ctx(Ctx::Controller)),
    ));

    let mut data = CreatureData::new("5UU", 3, 3);
    data.delve = true;
    let mut card = CardDef::new(
        "Murktide Regent",
        CardKind::Creature(data),
        parse_colors("5UU", true, false),
        Some(25),
        vec![], CardLayout::Normal, None,
        vec![],
        vec![ReplacementDef {
            check: Arc::new(murktide_etb_check),
            make_effect: Arc::new(|_source_id, controller: PlayerId| {
                Effect(Arc::new(move |state, t, targets| {
                    let Some(&id) = targets.first() else { return; };
                    // Count instants/sorceries exiled specifically as delve payment (CR 702.66b).
                    // `resolving_costs_ctx` is set by resolve_top_of_stack before the effect runs.
                    let delve_ids = state.resolving_costs_ctx.objects_moved.clone();
                    let exile_count = delve_ids.iter()
                        .filter(|&&id| {
                            state.objects.get(&id)
                                .and_then(|o| state.catalog.get(o.catalog_key.as_str()))
                                .map_or(false, |d| d.is_instant() || d.is_sorcery())
                        })
                        .count() as i32;
                    if let Some(bf) = state.permanent_bf_mut(id) {
                        bf.counters = exile_count;
                    }
                    fire_event(
                        GameEvent::ZoneChange {
                            id,
                            actor: controller,
                            from: ZoneId::Stack,
                            to: ZoneId::Battlefield,
                            controller,
                        },
                        state, t, controller,
                    );
                }))
            }),
            active_when: tp_always(),
        }],
        vec![],
        vec![],
    );
    // IR: "Whenever an instant or sorcery card is put into exile from your
    // graveyard, put a +1/+1 counter on Murktide Regent."
    card.abilities = vec![Ability {
        kind: AbilityKind::Triggered {
            spec: TriggerSpec::When {
                pattern: EventPattern::ZoneChange {
                    obj_filter: is_inst_or_sorc,
                    from: ZoneKindSel::Graveyard,
                    to: ZoneKindSel::Exile,
                    actor_filter: Some(actor_is_you),
                },
                condition: None,
            },
            target_spec: TargetSpec::None,
            body: Action::PutCounters {
                on: Expr::Ctx(Ctx::Source),
                kind: CounterType::PlusOnePlusOne,
                n: Expr::Num(1),
            },
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("Whenever an instant or sorcery card is put into exile from your graveyard, put a +1/+1 counter on Murktide Regent."),
    }];
    card
}

/// Shadow (evasion — see strategy.rs), replacement effect (opponent's GY-bound cards
/// exile with a void counter), and {T}, SacSelf activated ability (choose an exiled
/// opponent card with a void counter; grant a free-cast permission for it this turn).
/// CR 702.28 (shadow), CR 614.1a (replacement).
fn dauthi_voidwalker() -> CardDef {
    use crate::ir::ability::{
        Ability, AbilityKind, CostBody, EventPattern, ReplacementBody,
    };
    use crate::ir::action::{Action, Expiry as IrExpiry};
    use crate::ir::ce::{CEMod, CostSpec};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};

    let mut data = CreatureData::new("BB", 3, 2);
    data.keywords = Keywords::from_slice(&[Keyword::Shadow]);

    // "A card an opponent owns" — in practice the moving card's controller
    // differs from DV's controller (for the zones this fires in, controller
    // tracks owner).
    let opp_card = Filter(Expr::Not(Box::new(Expr::Eq(
        Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::It)))),
        Box::new(Expr::Ctx(Ctx::Controller)),
    ))));

    let exile_with_void = Ability {
        kind: AbilityKind::Replacement {
            matches: EventPattern::EntersZone {
                obj_filter: opp_card,
                zone_kind: ZoneKindSel::Graveyard,
            },
            condition: None,
            body: ReplacementBody::Replace(Action::Sequence(vec![
                Action::Move {
                    what: Expr::Ctx(Ctx::Var("triggered_obj")),
                    to: ZoneKindSel::Exile,
                    to_owner: None,
                    bind_as: None,
                },
                Action::PutCounters {
                    on: Expr::Ctx(Ctx::Var("triggered_obj")),
                    kind: CounterType::Void,
                    n: Expr::Num(1),
                },
            ])),
        },
        text: Some(
            "If a card an opponent owns would be put into a graveyard from anywhere, \
             instead exile it with a void counter on it.",
        ),
    };

    let may_play = Ability {
        kind: AbilityKind::Activated {
            // Phase 4 step 3: TapSelf+SacSelf conjunction migrated to IR.
            cost: CostBody::Ir(Action::Sequence(vec![
                Action::Tap { target: Expr::Ctx(Ctx::Source) },
                Action::Sacrifice {
                    who: crate::ir::action::Who::You,
                    filter: Filter(Expr::Eq(
                        Box::new(Expr::Ctx(Ctx::It)),
                        Box::new(Expr::Ctx(Ctx::Source)),
                    )),
                    count: Expr::Num(1),
                    bind_as: None,
                },
            ])),
            target_spec: TargetSpec::None,
            choice_spec: Some(ChoiceSpec {
                controller: Who::Opp,
                zone: ZoneId::Exile,
                filter: ir_has_counter(CounterType::Void),
            }),
            body: Action::ApplyCE {
                target: Expr::Ctx(Ctx::Var("target")),
                mods: vec![
                    CEMod::CastableFrom(ZoneKindSel::Exile),
                    CEMod::AltCost(CostSpec::Free),
                ],
                expiry: IrExpiry::EndOfTurn,
            },
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: crate::ir::expr::ZoneKindSel::Battlefield,
        },
        text: Some(
            "{T}, Sacrifice ~: Choose an exiled card an opponent owns with a void counter \
             on it. You may play it this turn without paying its mana cost.",
        ),
    };

    let mut card = CardDef::new(
        "Dauthi Voidwalker",
        CardKind::Creature(data),
        parse_colors("BB", false, false),
        None,
        vec![],
        CardLayout::Normal,
        None,
        vec![],
        vec![],
        vec![],
        vec![],
    );
    card.abilities = vec![exile_with_void, may_play];
    card
}

/// Prohibition: each opponent can't cast noncreature spells with MV > their land count.
/// Trigger: whenever an opponent casts a spell with no mana spent, counter it.
/// CR 614.17 (prohibition), CR 603 (trigger).
fn lavinia_azorius_renegade() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, ActionKind, EventPattern, TriggerSpec};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel, ZoneSel};

    let mut data = CreatureData::new("WU", 2, 2);
    data.legendary = true;
    let mut card = CardDef::new(
        "Lavinia, Azorius Renegade",
        CardKind::Creature(data),
        parse_colors("WU", true, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![],
        vec![],
        vec![],
        vec![],  // "can't cast" is now an AbilityKind::Restriction (card.abilities below)
    );

    // "Each opponent can't cast noncreature spells with mana value greater than the
    // number of lands that player controls." An action-Restriction consulted at
    // legal-cast enumeration (AND-NOT over castable → "can't beats can", CR 101.2).
    // subject = the proposed spell (Ctx::It): opponent-controlled ∧ noncreature ∧
    // MV > (lands its controller controls).
    let lands_controlled = Expr::Count(Box::new(Expr::AllObjects {
        zone: ZoneSel::Scoped {
            zone_kind: ZoneKindSel::Battlefield,
            owner: Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::It)))),
        },
        bind: "l",
        filter: Box::new(Expr::Contains(
            Box::new(Expr::TypeLit(CardType::Land)),
            Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::Var("l"))))),
        )),
    }));
    let cant_cast = Ability {
        kind: AbilityKind::Restriction {
            action: ActionKind::Cast,
            subject: Filter(Expr::And(
                Box::new(Expr::Not(Box::new(Expr::Eq(
                    Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::It)))),
                    Box::new(Expr::Ctx(Ctx::Controller)),
                )))),
                Box::new(Expr::And(
                    Box::new(Expr::Not(Box::new(Expr::Contains(
                        Box::new(Expr::TypeLit(CardType::Creature)),
                        Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::It)))),
                    )))),
                    Box::new(Expr::Gt(
                        Box::new(Expr::Mv(Box::new(Expr::Ctx(Ctx::It)))),
                        Box::new(lands_controlled),
                    )),
                )),
            )),
        },
        text: Some("Each opponent can't cast noncreature spells with mana value greater than the number of lands that player controls."),
    };

    // Trigger: "Whenever an opponent casts a spell, if no mana was spent to
    // cast it, counter that spell."
    let opp_cast_free = Expr::And(
        Box::new(Expr::Not(Box::new(Expr::Eq(
            Box::new(Expr::Ctx(Ctx::Var("triggered_actor"))),
            Box::new(Expr::Ctx(Ctx::Controller)),
        )))),
        Box::new(Expr::Not(Box::new(Expr::Ctx(Ctx::Var("triggered_mana_spent"))))),
    );
    card.abilities = vec![
        cant_cast,
        Ability {
            kind: AbilityKind::Triggered {
                spec: TriggerSpec::When {
                    pattern: EventPattern::SpellCast { spell_filter: Filter(Expr::Bool(true)) },
                    condition: Some(opp_cast_free),
                },
                target_spec: TargetSpec::None,
                body: Action::Counter {
                    target: Expr::Ctx(Ctx::Var("triggered_obj")),
                },
                active_zone: ZoneKindSel::Battlefield,
            },
            text: Some("Whenever an opponent casts a spell, if no mana was spent to cast it, counter that spell."),
        },
    ];
    card
}

/// Phelia, Exuberant Shepherd — {1}{W} Legendary Creature — Dog (2/2)
/// Flash.
/// Whenever Phelia attacks, exile up to one other target nonland permanent. At the
/// beginning of the next end step, return that card to the battlefield under its
/// owner's control. If it entered under your control, put a +1/+1 counter on Phelia.
///
/// "Entered under your control" ≡ the exiled card's owner is Phelia's controller
/// (since returns go to owner). Blinking your own permanent grows Phelia; blinking
/// an opponent's does not.
///
/// Attack trigger fires on `EnteredStep { DeclareAttackers }` gated by
/// `permanent_bf(src).attacking` (same pattern as Tamiyo). "Up to one" is modeled
/// via `TargetSpec::Union` of Actor+Opp nonland permanents; pick_targets returns
/// at most one; effect no-ops if `targets` is empty. Delayed return is a floating
/// `TriggerInstance` with `Expiry::OneShot` firing on `EnteredStep { End }` (same
/// pattern as Sneak Attack). Controller is reset to owner on return (CR 614 return
/// to battlefield under owner's control).
fn phelia_exuberant_shepherd() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, EventPattern, StepScope, TriggerSpec};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};

    let mut data = CreatureData::new("1W", 2, 2);
    data.legendary = true;
    data.creature_subtypes = vec!["Dog".into()];
    data.keywords.insert(Keyword::Flash);

    // Nonland permanent, not this Phelia itself ("up to one other").
    let nonland_other = ir_not(ir_type(CardType::Land));
    let nonland_other_for_filter = nonland_other.clone();
    let target_spec = TargetSpec::Union(vec![
        TargetSpec::ObjectInZone {
            controller: Who::Actor, zone: ZoneId::Battlefield,
            filter: {
                // Exclude self at pick time via a wrapping filter that calls the inner.
                // Since TargetSpec::ObjectInZone doesn't see source_id, we rely on the
                // strategy's target filter to skip self; legacy filter did the same by
                // capturing `src`. Here we approximate: exclude via a filter closure
                // bound at target-legality time via legal_targets (which has source_id).
                nonland_other.clone()
            },
        },
        TargetSpec::ObjectInZone {
            controller: Who::Opp, zone: ZoneId::Battlefield,
            filter: nonland_other_for_filter,
        },
    ]);

    let mut card = CardDef::new(
        "Phelia, Exuberant Shepherd",
        CardKind::Creature(data),
        parse_colors("1W", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![],
        vec![], vec![], vec![],
    );

    // Filter: the attacker is this Phelia.
    let self_attacks = Filter(Expr::Eq(
        Box::new(Expr::Ctx(Ctx::It)),
        Box::new(Expr::Ctx(Ctx::Source)),
    ));

    // Delayed-trigger body (runs at next end step):
    //   Move exiled card back to battlefield under its owner's control;
    //   if it returns under Phelia's controller (owner == you), +1/+1 counter.
    let delayed_body = Action::Sequence(vec![
        Action::Move {
            what: Expr::Ctx(Ctx::Var("blinked")),
            to: ZoneKindSel::Battlefield,
            to_owner: Some(Expr::Owner(Box::new(Expr::Ctx(Ctx::Var("blinked"))))),
            bind_as: None,
        },
        Action::IfThen {
            cond: Expr::And(
                Box::new(Expr::Bound("blinked")),
                Box::new(Expr::Eq(
                    Box::new(Expr::Owner(Box::new(Expr::Ctx(Ctx::Var("blinked"))))),
                    Box::new(Expr::Ctx(Ctx::Controller)),
                )),
            ),
            then: Box::new(Action::PutCounters {
                on: Expr::Ctx(Ctx::Source),
                kind: CounterType::PlusOnePlusOne,
                n: Expr::Num(1),
            }),
            else_: None,
        },
    ]);

    // Attack-trigger body: exile target (if any), schedule delayed return.
    let body = Action::Sequence(vec![
        Action::Exile {
            target: Expr::Ctx(Ctx::Var("target")),
            bind_as: Some("blinked"),
        },
        Action::ScheduleDelayedTrigger {
            fires: TriggerSpec::AtStep {
                step: StepKind::End,
                who: StepScope::EachPlayer,
            },
            action: Box::new(delayed_body),
        },
    ]);

    card.abilities = vec![Ability {
        kind: AbilityKind::Triggered {
            spec: TriggerSpec::When {
                pattern: EventPattern::Attacks { attacker_filter: self_attacks },
                condition: None,
            },
            target_spec,
            body,
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("Whenever Phelia, Exuberant Shepherd attacks, exile up to one other target nonland permanent. At the beginning of the next end step, return that card to the battlefield under its owner's control. If it entered under your control, put a +1/+1 counter on Phelia."),
    }];
    card
}

/// {1}{R}, 2/2 Goblin Sorcerer.
/// "This spell can't be countered." — ProhibitionDef active while on stack.
/// "Ward—Pay 2 life." — TriggerCheckFn on SpellCast, checks spell's chosen_targets.
/// "Spells you control can't be countered." — ProhibitionDef active while on battlefield.
/// "Other creatures you control have Ward—Pay 2 life." — second TriggerCheckFn (approximation;
///   see TODO below for the true CE-layer-6 implementation).
fn hexing_squelcher() -> CardDef {
    let data = CreatureData::new("1R", 2, 2);
    CardDef::new(
        "Hexing Squelcher",
        CardKind::Creature(data),
        parse_colors("R", false, false),
        None,
        vec![], CardLayout::Normal, None,
        // Ward—Pay 2 life: fires when an opponent's spell targets this permanent.
        vec![TriggerDef { check: Arc::new(|event, source_id, controller, state, pending| {
            if let GameEvent::SpellCast { caster, card_id, .. } = event {
                if *caster == controller { return; }
                let is_targeted = state.objects.get(card_id)
                    .and_then(|o| o.spell())
                    .map_or(false, |s| s.chosen_targets.contains(&source_id));
                if is_targeted {
                    let spell_id = *card_id;
                    let targeting_caster = *caster;
                    pending.push(TriggerContext {
                        source_name: "Hexing Squelcher (Ward)".into(),
                        controller,
                        target_spec: TargetSpec::None,
                        effect: Effect(Arc::new(move |state, t, _| {
                            ward_pay_or_counter(
                                source_id,
                                &crate::ir::action::Action::PayLife { who: crate::ir::action::Who::You, amount: crate::ir::expr::Expr::Num(2) },
                                spell_id,
                                targeting_caster,
                                controller,
                                state,
                                t,
                            );
                        })),
                    });
                }
            }
        }), active_when: tp_on_battlefield() }],
        vec![],
        vec![
            // "Spells you control can't be countered." (while on battlefield)
            ProhibitionDef {
                check: Arc::new(|event, _source_id, controller, _state| {
                    matches!(event, GameEvent::SpellBeingCountered { caster, .. } if *caster == controller)
                }),
                active_when: tp_on_battlefield(),
            },
            // "This spell can't be countered." (while on stack)
            ProhibitionDef {
                check: Arc::new(|event, source_id, _controller, _state| {
                    matches!(event, GameEvent::SpellBeingCountered { card_id, .. } if *card_id == source_id)
                }),
                active_when: tp_on_stack(),
            },
        ],
        // "Other creatures you control have Ward—Pay 2 life."
        // L6 CE: while Hexing Squelcher is on the battlefield, push a Ward trigger into each
        // other creature controlled by the same player via granted_trigger_defs.
        vec![Arc::new(move |source_id, controller| ContinuousInstance {
            source_id,
            controller,
            layer: ContinuousLayer::L6AbilityEffects,
            reads: vec![],
            writes: vec![CeWrites::Abilities],
            timestamp: 0, // assigned at ETB via ci_timestamp
            filter: Arc::new(move |id, ctr, _| ctr == controller && id != source_id),
            modifier: Arc::new(|def, _state| {
                if matches!(def.kind, CardKind::Creature(_)) {
                    def.granted_trigger_defs.push(Arc::new(
                        |event, source_id, controller, state, pending| {
                            if let GameEvent::SpellCast { caster, card_id, .. } = event {
                                if *caster == controller { return; }
                                let is_targeted = state.objects.get(card_id)
                                    .and_then(|o| o.spell())
                                    .map_or(false, |s| s.chosen_targets.contains(&source_id));
                                if is_targeted {
                                    let spell_id = *card_id;
                                    let targeting_caster = *caster;
                                    pending.push(TriggerContext {
                                        source_name: "Hexing Squelcher (Ward grant)".into(),
                                        controller,
                                        target_spec: TargetSpec::None,
                                        effect: Effect(Arc::new(move |state, t, _| {
                                            ward_pay_or_counter(
                                                source_id,
                                                &crate::ir::action::Action::PayLife { who: crate::ir::action::Who::You, amount: crate::ir::expr::Expr::Num(2) },
                                                spell_id,
                                                targeting_caster,
                                                controller,
                                                state,
                                                t,
                                            );
                                        })),
                                    });
                                }
                            }
                        },
                    ));
                }
            }),
            expiry: Expiry::WhileSourceOnBattlefield,

        })],
    )
}

// ── DFCs / split cards ────────────────────────────────────────────────────────

/// Front: 0/3 creature for {U}, generates Clue tokens when it attacks.
/// Back: Tamiyo, Seasoned Scholar — planeswalker with +2 loyalty ability.
/// Transforms after controller draws their 3rd card in a turn. CR 701.28.
fn tamiyo_inquisitive_student() -> CardDef {
    let back = CardDef::new(
        "Tamiyo, Seasoned Scholar",
        CardKind::Planeswalker(PlaneswalkerData {
            mana_cost: String::new(),
            loyalty: 2,
            abilities: vec![
                AbilityDef {
                    costs: ir_loyalty(2),
                    ability_factory: Some(Arc::new(build_tamiyo_plus_two)),
                    timing: ActivationTiming::Sorcery,
                    ..Default::default()
                },
                AbilityDef {
                    costs: ir_loyalty(-3),
                    ability_factory: Some(Arc::new(build_tamiyo_minus_three)),
                    target_spec: TargetSpec::ObjectInZone {
                        controller: Who::Actor,
                        zone: ZoneId::Graveyard,
                        filter: ir_or(ir_type(CardType::Instant), ir_type(CardType::Sorcery)),
                    },
                    timing: ActivationTiming::Sorcery,
                    ..Default::default()
                },
                AbilityDef {
                    costs: ir_loyalty(-7),
                    ability_factory: Some(Arc::new(build_tamiyo_minus_seven)),
                    timing: ActivationTiming::Sorcery,
                    ..Default::default()
                },
            ],
        }),
        parse_colors("U", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![],
        vec![replacement_planeswalker_etb(2)],
        vec![],
        vec![],
    );

    let mut front_data = CreatureData::new("U", 0, 3);
    front_data.legendary = true;

    CardDef::new(
        "Tamiyo, Inquisitive Student",
        CardKind::Creature(front_data),
        parse_colors("U", false, false),
        None,
        vec![Supertype::Legendary], CardLayout::DoubleFaced, Some(Box::new(back)),
        vec![TriggerDef { check: Arc::new(tamiyo_check), active_when: tp_on_battlefield() }],
        vec![],
        vec![],
        vec![],
    )
}

/// Artifact Creature {2}, 1/3. ETB: choose a color; all objects everywhere gain that color.
/// Layer 5 continuous effect, expires when Painter leaves the battlefield.
/// CR 613.4 (color-changing effects apply at layer 5).
fn painters_servant() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;

    // ETB (CR 614.12 "as ~ enters, choose a color") — store the choice only.
    let repl = etb_self_replacement(|source_id, id, controller, state, _t| {
        let ChoiceResult::Color(chosen) =
            state.with_strategy(controller, |s, st| s.resolve_choice(source_id, &ChoiceRequest::Color, st)) else { return };
        if let Some(bf) = state.permanent_bf_mut(id) {
            bf.etb_choice = Some(ChoiceResult::Color(chosen));
        }
    });
    let mut def = CardDef::new(
        "Painter's Servant",
        CardKind::Creature(CreatureData::new("2", 1, 3)),
        vec![],
        Some(40),
        vec![], CardLayout::Normal, None,
        vec![],
        vec![repl],
        vec![],
        vec![],
    );
    // "All cards, spells, and permanents are the chosen color in addition to their
    // other colors." An L5 IR Static (scope None = every object) that adds the color
    // read from this Painter's own ETB choice. Expires with the source by default.
    def.abilities = vec![Ability {
        kind: AbilityKind::Static {
            mods: vec![CEMod::AddColor(Expr::ChosenColor(Box::new(Expr::Ctx(Ctx::Source))))],
            scope: None,
            condition: None,
        },
        text: Some("All cards, spells, and permanents are the chosen color in addition to their other colors."),
    }];
    // Painter's Servant is an Artifact Creature; the constructor derives only one type from
    // CardKind, so we push the second type explicitly.
    def.types.push(CardType::Artifact);
    def
}

/// Enchantment for {2BB}. Replacement: any card going to any graveyard goes to exile instead.
fn leyline_of_the_void() -> CardDef {
    let replacement = ReplacementDef {
        check: Arc::new(leyline_check),
        make_effect: Arc::new(|_source_id, controller: PlayerId| {
            Effect(Arc::new(move |state, t, targets| {
                if let Some(&id) = targets.first() {
                    change_zone(id, ZoneId::Exile, state, t, controller);
                }
            }))
        }),
        active_when: tp_on_battlefield(),
    };
    CardDef::new(
        "Leyline of the Void",
        CardKind::Enchantment(EnchantmentData::default()),
        parse_colors("2BB", false, true),
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![replacement], vec![], vec![],
    )
}

/// Flash, colorless artifact for {2}.
/// As this enters, choose a card name. Spells with that name cost {3} more to cast.
/// Activated abilities of sources with that name can't be activated unless they're mana abilities.
fn disruptor_flute() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, ActionKind};
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};

    let mut card = CardDef::new(
        "Disruptor Flute",
        CardKind::Artifact(ArtifactData {
            mana_cost: "2".to_string(),
            ..Default::default()  // no activated abilities
        }),
        vec![],  // colorless
        Some(40),
        vec![], CardLayout::Normal, None,
        vec![],  // no trigger_defs
        // ETB (CR 614.12 "as ~ enters, choose a card name") — store the choice in
        // etb_choice; the ongoing effects below read it. No CE pushed here anymore.
        vec![etb_self_replacement(|source_id, id, controller, state, _t| {
            let ChoiceResult::CardName(chosen) =
                state.with_strategy(controller, |s, st| s.resolve_choice(source_id, &ChoiceRequest::CardName, st)) else { return };
            if let Some(bf) = state.permanent_bf_mut(id) {
                bf.etb_choice = Some(ChoiceResult::CardName(chosen));
            }
        })],
        vec![],  // no prohibition_defs
        vec![],  // ongoing effects are IR abilities (below)
    );

    // Both ongoing clauses scope to "a card whose name == the name this Flute
    // chose" — `Eq(Name(It), ChosenName(Source))`, evaluated per candidate with
    // the Flute bound as Source.
    let name_matches = || Expr::Eq(
        Box::new(Expr::Name(Box::new(Expr::Ctx(Ctx::It)))),
        Box::new(Expr::ChosenName(Box::new(Expr::Ctx(Ctx::Source)))),
    );

    card.abilities = vec![
        // "Spells with the chosen name cost {3} more to cast." A casting-cost
        // surcharge CE (recompute writes casting_cost_modifier += 3 on matches).
        Ability {
            kind: AbilityKind::Static {
                mods: vec![CEMod::CastingCostPlus(Expr::Num(3))],
                scope: Some(Filter(name_matches())),
                condition: None,
            },
            text: Some("Spells with the chosen name cost {3} more to cast."),
        },
        // "Activated abilities of sources with the chosen name can't be activated
        // unless they're mana abilities." An action-Restriction over the named card;
        // the "unless they're mana abilities" rider (CR 605.1a) is a subject clause
        // `Not(activating_mana_ability)` — the engine binds that bool while the mana
        // sub-loop is consulting — not a flag on the variant.
        Ability {
            kind: AbilityKind::Restriction {
                action: ActionKind::Activate,
                subject: Filter(Expr::And(
                    Box::new(name_matches()),
                    Box::new(Expr::Not(Box::new(Expr::Ctx(Ctx::Var("activating_mana_ability"))))),
                )),
            },
            text: Some("Activated abilities of sources with the chosen name can't be activated unless they're mana abilities."),
        },
    ];
    card
}

/// Legendary Planeswalker — Karn {4}. Loyalty 5.
/// Static: "Activated abilities of artifacts your opponents control can't be activated."
/// CE sets activatable=false on ALL abilities (AbilityDef + ManaAbility) of opponent-controlled artifacts.
/// +1 and −2 abilities are not modeled (not relevant to the Doomsday sim).
fn karn_the_great_creator() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, ActionKind};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};
    let mut card = CardDef::new(
        "Karn, the Great Creator",
        CardKind::Planeswalker(PlaneswalkerData {
            mana_cost: "4".to_string(),
            loyalty: 5,
            abilities: vec![],  // +1/−2 not modeled
        }),
        vec![],  // colorless
        None,
        vec![Supertype::Legendary], CardLayout::Normal, None,
        vec![],  // no triggers
        vec![replacement_planeswalker_etb(5)],
        vec![],  // no prohibitions
        vec![], // "can't activate" is now an AbilityKind::Restriction (card.abilities below)
    );
    // "Activated abilities of artifacts your opponents control can't be activated."
    // Asymmetric: subject = artifact ∧ controlled by an opponent of Karn's controller.
    card.abilities = vec![Ability {
        kind: AbilityKind::Restriction {
            action: ActionKind::Activate,
            subject: Filter(Expr::And(
                Box::new(Expr::Contains(
                    Box::new(Expr::TypeLit(CardType::Artifact)),
                    Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::It)))),
                )),
                Box::new(Expr::Not(Box::new(Expr::Eq(
                    Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::It)))),
                    Box::new(Expr::Ctx(Ctx::Controller)),
                )))),
            )),
        },
        text: Some("Activated abilities of artifacts your opponents control can't be activated."),
    }];
    card
}

/// IR ability: "Nonbasic lands are Mountains." Shared between Blood Moon and
/// Magus of the Moon. CR 305.6 / 305.7 / 613.1d. The scope filter is `None`:
/// the `SetBasicLandType` modifier is the sole gating point (non-lands and
/// basics short-circuit inside the modifier itself).
fn nonbasic_lands_are_mountains_ir() -> crate::ir::ability::Ability {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::ce::{BasicLandType, CEMod};
    Ability {
        kind: AbilityKind::Static {
            mods: vec![CEMod::SetBasicLandType(BasicLandType::Mountain)],
            scope: None,
            condition: None,
        },
        text: Some("Nonbasic lands are Mountains."),
    }
}

/// Enchantment {2R}. Static: "Nonbasic lands are Mountains." CR 305.7, 613.1d.
fn blood_moon() -> CardDef {
    let mut card = CardDef::new(
        "Blood Moon",
        CardKind::Enchantment(EnchantmentData::default()),
        parse_colors("2R", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![], vec![],
        vec![],
    );
    card.abilities = vec![nonbasic_lands_are_mountains_ir()];
    card
}

/// Creature {2R}, 2/2. Static: "Nonbasic lands are Mountains." CR 305.7, 613.1d.
fn magus_of_the_moon() -> CardDef {
    let data = CreatureData::new("2R", 2, 2);
    let mut card = CardDef::new(
        "Magus of the Moon",
        CardKind::Creature(data),
        parse_colors("2R", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![], vec![],
        vec![],
    );
    card.abilities = vec![nonbasic_lands_are_mountains_ir()];
    card
}

/// IR ability: "Each land is a <type> in addition to its other land types."
/// Shared between Urborg, Tomb of Yawgmoth and Yavimaya, Cradle of Growth.
/// CR 305.6 / 613.1d. No scope — the modifier's early-return for non-lands
/// is the sole filter.
fn each_land_is_also_ir(
    kind: crate::ir::ce::BasicLandType,
    text: &'static str,
) -> crate::ir::ability::Ability {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::ce::CEMod;
    Ability {
        kind: AbilityKind::Static {
            mods: vec![CEMod::AddBasicLandType(kind)],
            scope: None,
            condition: None,
        },
        text: Some(text),
    }
}

/// Legendary Land. "Each land is a Swamp in addition to its other land types."
/// Adds Swamp type and "{T}: Add {B}" to all lands. CR 305.7, 613.1d.
fn urborg_tomb_of_yawgmoth() -> CardDef {
    use crate::ir::ce::BasicLandType;
    let mut card = CardDef::new(
        "Urborg, Tomb of Yawgmoth",
        CardKind::Land(LandData::default()),
        vec![],
        None,
        vec![Supertype::Legendary], CardLayout::Normal, None,
        vec![], vec![], vec![],
        vec![],
    );
    card.abilities = vec![each_land_is_also_ir(
        BasicLandType::Swamp,
        "Each land is a Swamp in addition to its other land types.",
    )];
    card
}

/// Legendary Land. "Each land is a Forest in addition to its other land types."
/// Adds Forest type and "{T}: Add {G}" to all lands. CR 305.7, 613.1d.
fn yavimaya_cradle_of_growth() -> CardDef {
    use crate::ir::ce::BasicLandType;
    let mut card = CardDef::new(
        "Yavimaya, Cradle of Growth",
        CardKind::Land(LandData::default()),
        vec![],
        None,
        vec![Supertype::Legendary], CardLayout::Normal, None,
        vec![], vec![], vec![],
        vec![],
    );
    card.abilities = vec![each_land_is_also_ir(
        BasicLandType::Forest,
        "Each land is a Forest in addition to its other land types.",
    )];
    card
}

/// Land. "This land enters tapped unless you control a Mountain or a Forest."
/// {T}: Add {U}.
/// {U}, {T}: The next spell you cast this turn can't be countered. (CR 611.2f)
fn mistrise_village() -> CardDef {
    use crate::ir::ability::{
        Ability, AbilityKind, EventPattern, ReplacementBody,
    };
    use crate::ir::action::{Action, Expiry as IrExpiry, Who};
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel, ZoneSel};

    // ETB replacement: enters tapped unless you control a Mountain or Forest.
    // Condition (the replacement fires iff true): you control zero lands whose
    // subtypes include "mountain" or "forest" — evaluated on the materialized
    // view, so Yavimaya/Urborg-style CE effects are honored.
    let self_etb = Filter(Expr::Eq(
        Box::new(Expr::Ctx(Ctx::It)),
        Box::new(Expr::Ctx(Ctx::Source)),
    ));
    let controller_has_mountain_or_forest = Expr::AllObjects {
        zone: ZoneSel::Global(ZoneKindSel::Battlefield),
        bind: "p",
        filter: Box::new(Expr::And(
            Box::new(Expr::Eq(
                Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::Var("p"))))),
                Box::new(Expr::Ctx(Ctx::Controller)),
            )),
            Box::new(Expr::Or(
                Box::new(Expr::Contains(
                    Box::new(Expr::SubtypeLit("mountain".to_string())),
                    Box::new(Expr::Subtypes(Box::new(Expr::Ctx(Ctx::Var("p"))))),
                )),
                Box::new(Expr::Contains(
                    Box::new(Expr::SubtypeLit("forest".to_string())),
                    Box::new(Expr::Subtypes(Box::new(Expr::Ctx(Ctx::Var("p"))))),
                )),
            )),
        )),
    };
    let enters_tapped = Ability {
        kind: AbilityKind::Replacement {
            matches: EventPattern::EntersZone {
                obj_filter: self_etb,
                zone_kind: ZoneKindSel::Battlefield,
            },
            condition: Some(Expr::Eq(
                Box::new(Expr::Count(Box::new(controller_has_mountain_or_forest))),
                Box::new(Expr::Num(0)),
            )),
            body: ReplacementBody::Replace(Action::Sequence(vec![
                Action::Move {
                    what: Expr::Ctx(Ctx::Var("triggered_obj")),
                    to: ZoneKindSel::Battlefield,
                    to_owner: None,
                    bind_as: None,
                },
                Action::Tap { target: Expr::Ctx(Ctx::Var("triggered_obj")) },
            ])),
        },
        text: Some("~ enters tapped unless you control a Mountain or a Forest."),
    };

    // {U},{T}: The next spell you cast this turn can't be countered. (CR 611.2f)
    let next_spell_uncounterable = Ability {
        kind: AbilityKind::Activated {
            cost: ir_seq(vec![act_pay_mana_str("U"), act_tap_source()]),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::GrantCEToNextSpellCast {
                who: Who::You,
                predicate: None,
                mods: vec![CEMod::Uncounterable],
                expiry: IrExpiry::EndOfTurn,
            },
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: crate::ir::expr::ZoneKindSel::Battlefield,
        },
        text: Some("{U}, {T}: The next spell you cast this turn can't be countered."),
    };

    let mut card = CardDef::new(
        "Mistrise Village",
        CardKind::Land(LandData {
            mana_abilities: vec![tap_produces("U")],
            ..Default::default()
        }),
        vec![], None, vec![], CardLayout::Normal, None,
        vec![],
        vec![],
        vec![],
        vec![],
    );
    card.abilities = vec![enters_tapped, next_spell_uncounterable];
    card
}

/// Great Furnace — Artifact Land. {T}: Add {R}.
/// Primary kind is Land; additionally typed as Artifact (for Brotherhood's End, etc.).
fn great_furnace() -> CardDef {
    let mut def = simple("Great Furnace", CardKind::Land(LandData {
        mana_abilities: vec![],
        ..Default::default()
    }), vec![], None);
    def.types.push(CardType::Artifact);
    def.abilities.push(ir_tap_mana("R"));
    def
}

/// Front: Brazen Borrower — 3/1 flying creature for {1UU}.
/// Back (adventure): Petty Theft — instant for {1U}, bounce a nonland permanent. CR 715.
// ── Tokens ────────────────────────────────────────────────────────────────────

/// 0/0 Orc Army creature token. Created and grown by Amass Orcs. CR 701.45.
fn orc_army_token() -> CardDef {
    simple("Orc Army", CardKind::Creature(CreatureData::new("", 0, 0)), vec![], None)
}

/// Colorless Clue artifact token. Activated ability: {2}, tap self, sacrifice self → draw one.
/// CR 701.28 (Investigate).
fn clue_token() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::action::{Action, Who as IrWho};
    use crate::ir::expr::Expr;

    let mut card = simple(
        "Clue Token",
        CardKind::Artifact(ArtifactData {
            mana_cost: String::new(),
            ..Default::default()
        }),
        vec![],
        None,
    );
    card.abilities = vec![Ability {
        kind: AbilityKind::Activated {
            cost: ir_seq(vec![
                act_pay_mana_str("2"),
                act_tap_source(),
                act_sac_self("$clue_self"),
            ]),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::Draw {
                who: IrWho::You,
                n: Expr::Num(1),
            },
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: crate::ir::expr::ZoneKindSel::Battlefield,
        },
        text: Some("{2}, {T}, Sacrifice this artifact: Draw a card."),
    }];
    card
}

/// 1/1 white Monk creature token with prowess.
/// Prowess: "Whenever you cast a noncreature spell, this creature gets +1/+1 until end of turn."
fn monk_token() -> CardDef {
    use crate::ir::action::{Action, Expiry as IrExpiry};
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    CardDef::new(
        "Monk Token",
        CardKind::Creature(CreatureData::new("", 1, 1)),
        vec![Color::White],
        None,
        vec![], CardLayout::Normal, None,
        // Prowess: whenever controller casts a noncreature spell, +1/+1 until EOT.
        vec![TriggerDef {
            check: Arc::new(|event, source_id, controller, state, pending| {
                if let GameEvent::SpellCast { caster, card_id, .. } = event {
                    if *caster != controller { return; }
                    let is_creature = state.objects.get(card_id)
                        .and_then(|o| state.catalog.get(&o.catalog_key))
                        .map_or(false, |d| d.types.contains(&CardType::Creature));
                    if !is_creature {
                        pending.push(TriggerContext {
                            source_name: "Monk Token (prowess)".into(),
                            controller,
                            target_spec: TargetSpec::None,
                            // Self-pump +1/+1 until EOT via the unified CE path.
                            effect: eff_ir_targeted(controller, source_id, Action::ApplyCE {
                                target: Expr::Ctx(Ctx::Source),
                                mods: vec![CEMod::PumpPT(Expr::Num(1), Expr::Num(1))],
                                expiry: IrExpiry::EndOfTurn,
                            }),
                        });
                    }
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![], vec![],
    )
}

/// 0/0 black Phyrexian Germ creature token. Created by Living Weapon equipment (CR 702.92).
fn phyrexian_germ_token() -> CardDef {
    let mut data = CreatureData::new("", 0, 0);
    data.creature_subtypes = vec!["Phyrexian".into(), "Germ".into()];
    simple("Phyrexian Germ", CardKind::Creature(data), vec![Color::Black], None)
}

/// 2/2 colorless face-down creature token produced by the "cloak" keyword (CR 702.169).
/// ABNORMAL: real cloak puts the actual top card of library onto the battlefield face-down
/// (still that specific card, just hidden/characteristic-stripped), and grants ward {2} and
/// the ability to turn face up for its mana cost if it's a creature card. We model this as a
/// plain 2/2 token — ward/turn-face-up/identity-as-top-card are all omitted. Use anywhere a
/// cloaked permanent is needed (e.g. Cryptic Coat).
fn mysterious_creature_token() -> CardDef {
    let data = CreatureData::new("", 2, 2);
    simple("Mysterious Creature", CardKind::Creature(data), vec![], None)
}

/// Cori-Steel Cutter — {1}{R} Artifact — Equipment.
/// "Equipped creature gets +1/+1 and has trample and haste."
/// "Flurry — Whenever you cast your second spell each turn, create a 1/1 white Monk
///  creature token with prowess. You may attach this Equipment to it."
/// "Equip {1}{R}"
/// Build an `AbilityKind::Static` continuous effect scoped to the creature this
/// source (an Equipment) is attached to — `Eq(It, AttachedTo(Source))`. Each
/// `CEMod` becomes one CI at its own CR-613 layer (P/T pumps → L7, keyword grants
/// → L6) via the shared `cemod_to_modifier`. Replaces the per-equipment
/// hand-rolled `ContinuousInstance` closures.
fn equipped_creature_ce(mods: Vec<crate::ir::ce::CEMod>, text: &'static str) -> crate::ir::ability::Ability {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};
    Ability {
        kind: AbilityKind::Static {
            mods,
            scope: Some(Filter(Expr::Eq(
                Box::new(Expr::Ctx(Ctx::It)),
                Box::new(Expr::AttachedTo(Box::new(Expr::Ctx(Ctx::Source)))),
            ))),
            condition: None,
        },
        text: Some(text),
    }
}

fn cori_steel_cutter() -> CardDef {
    use crate::ir::ce::CEMod;
    use crate::ir::expr::Expr;
    let mut card = CardDef::new(
        "Cori-Steel Cutter",
        CardKind::Artifact(ArtifactData {
            mana_cost: "1R".to_string(),
            subtypes: vec!["Equipment".into()],
            abilities: vec![AbilityDef {
                // Equip {1}{R} — sorcery-speed, targets a creature you control.
                costs: ir_pay_mana_str("1R"),
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Actor,
                    zone: ZoneId::Battlefield,
                    filter: ir_type(CardType::Creature),
                },
                ability_factory: Some(Arc::new(|who, source_id| {
                    Effect(Arc::new(move |state, _t, targets| {
                        let Some(&creature_id) = targets.first() else { return };
                        do_attach(state, who, source_id, creature_id);
                    }))
                })),
                timing: ActivationTiming::Sorcery,
                ..Default::default()
            }],
            mana_abilities: vec![],
        }),
        parse_colors("R", false, false),
        None,
        vec![], CardLayout::Normal, None,
        // Flurry: whenever controller casts their second spell each turn, create Monk + may attach.
        vec![TriggerDef {
            check: Arc::new(|event, source_id, controller, state, pending| {
                if let GameEvent::SpellCast { caster, .. } = event {
                    if *caster != controller { return; }
                    // spells_cast_this_turn is incremented AFTER SpellCast fires,
                    // so == 1 means the first spell was counted and this is the second.
                    if state.player(controller).spells_cast_this_turn != 1 { return; }
                    pending.push(TriggerContext {
                        source_name: "Cori-Steel Cutter (flurry)".into(),
                        controller,
                        target_spec: TargetSpec::None,
                        effect: Effect(Arc::new(move |state, t, _targets| {
                            let token_id = do_create_token("Monk Token", controller, state, t);
                            // "You may attach this Equipment to it."
                            let choice = state.with_strategy(controller, |s, st| s.resolve_choice(source_id, &ChoiceRequest::MayAttach, st));
                            if matches!(choice, ChoiceResult::Bool(true)) {
                                do_attach(state, controller, source_id, token_id);
                            }
                        })),
                    });
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![],
        vec![], // static CE now IR (card.abilities below)
    );
    // Equipped creature gets +1/+1 and has trample and haste.
    card.abilities = vec![equipped_creature_ce(
        vec![
            CEMod::PumpPT(Expr::Num(1), Expr::Num(1)),
            CEMod::AddKeyword(Keyword::Trample),
            CEMod::AddKeyword(Keyword::Haste),
        ],
        "Equipped creature gets +1/+1 and has trample and haste.",
    )];
    card
}

/// Batterskull — {5} Artifact — Equipment.
/// "Living weapon (When this Equipment enters, create a 0/0 black Phyrexian Germ
///  creature token, then attach this to it.)"
/// "Equipped creature gets +4/+4 and has vigilance and lifelink."
/// "{3}: Return this Equipment to its owner's hand."
/// "Equip {5}"
fn batterskull() -> CardDef {
    use crate::ir::ce::CEMod;
    use crate::ir::expr::Expr;
    let mut card = CardDef::new(
        "Batterskull",
        CardKind::Artifact(ArtifactData {
            mana_cost: "5".to_string(),
            subtypes: vec!["Equipment".into()],
            abilities: vec![
                // {3}: Return this Equipment to its owner's hand.
                AbilityDef {
                    costs: ir_pay_mana_str("3"),
                    ability_factory: Some(Arc::new(|who, source_id| {
                        Effect(Arc::new(move |state, t, _targets| {
                            let owner = state.objects.get(&source_id).map(|o| o.owner).unwrap_or(who);
                            change_zone(source_id, ZoneId::Hand, state, t, owner);
                            state.log(t, who, "Batterskull → bounced to hand".to_string());
                        }))
                    })),
                    ..Default::default()
                },
                // Equip {5} — sorcery-speed, targets a creature you control.
                AbilityDef {
                    costs: ir_pay_mana_str("5"),
                    target_spec: TargetSpec::ObjectInZone {
                        controller: Who::Actor,
                        zone: ZoneId::Battlefield,
                        filter: ir_type(CardType::Creature),
                    },
                    ability_factory: Some(Arc::new(|who, source_id| {
                        Effect(Arc::new(move |state, _t, targets| {
                            let Some(&creature_id) = targets.first() else { return };
                            do_attach(state, who, source_id, creature_id);
                        }))
                    })),
                    timing: ActivationTiming::Sorcery,
                    ..Default::default()
                },
            ],
            mana_abilities: vec![],
        }),
        vec![], None,
        vec![], CardLayout::Normal, None,
        // Living weapon ETB: create a Phyrexian Germ token, then attach this to it.
        vec![TriggerDef {
            check: Arc::new(|event, source_id, controller, _state, pending| {
                if let GameEvent::ZoneChange { id, to: ZoneId::Battlefield, controller: ctlr, .. } = event {
                    if *id == source_id && *ctlr == controller {
                        pending.push(TriggerContext {
                            source_name: "Batterskull (living weapon)".into(),
                            controller,
                            target_spec: TargetSpec::None,
                            effect: Effect(Arc::new(move |state, t, _targets| {
                                let token_id = do_create_token("Phyrexian Germ", controller, state, t);
                                do_attach(state, controller, source_id, token_id);
                            })),
                        });
                    }
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![],
        vec![], // static CE now IR (card.abilities below)
    );
    // Equipped creature gets +4/+4 and has vigilance and lifelink.
    card.abilities = vec![equipped_creature_ce(
        vec![
            CEMod::PumpPT(Expr::Num(4), Expr::Num(4)),
            CEMod::AddKeyword(Keyword::Vigilance),
            CEMod::AddKeyword(Keyword::Lifelink),
        ],
        "Equipped creature gets +4/+4 and has vigilance and lifelink.",
    )];
    card
}

/// Meteor Sword — {7} Artifact — Equipment.
/// "When this Equipment enters, destroy target permanent."
/// "Equipped creature gets +3/+3."
/// "Equip {3}"
fn meteor_sword() -> CardDef {
    use crate::ir::action::Action;
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let any_permanent_target = TargetSpec::Union(vec![
        TargetSpec::ObjectInZone {
            controller: Who::Actor,
            zone: ZoneId::Battlefield,
            filter: ir_any(),
        },
        TargetSpec::ObjectInZone {
            controller: Who::Opp,
            zone: ZoneId::Battlefield,
            filter: ir_any(),
        },
    ]);
    let mut card = CardDef::new(
        "Meteor Sword",
        CardKind::Artifact(ArtifactData {
            mana_cost: "7".to_string(),
            subtypes: vec!["Equipment".into()],
            abilities: vec![AbilityDef {
                // Equip {3} — sorcery-speed, targets a creature you control.
                costs: ir_pay_mana_str("3"),
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Actor,
                    zone: ZoneId::Battlefield,
                    filter: ir_type(CardType::Creature),
                },
                ability_factory: Some(Arc::new(|who, source_id| {
                    Effect(Arc::new(move |state, _t, targets| {
                        let Some(&creature_id) = targets.first() else { return };
                        do_attach(state, who, source_id, creature_id);
                    }))
                })),
                timing: ActivationTiming::Sorcery,
                ..Default::default()
            }],
            mana_abilities: vec![],
        }),
        vec![], None,
        vec![], CardLayout::Normal, None,
        // ETB: destroy target permanent.
        vec![etb_self_trigger("Meteor Sword", any_permanent_target,
            |source_id, controller| eff_ir_targeted(controller, source_id,
                Action::Destroy { target: Expr::Ctx(Ctx::Var("target")) }))],
        vec![], vec![],
        vec![], // static CE now IR (card.abilities below)
    );
    // Equipped creature gets +3/+3.
    card.abilities = vec![equipped_creature_ce(
        vec![CEMod::PumpPT(Expr::Num(3), Expr::Num(3))],
        "Equipped creature gets +3/+3.",
    )];
    card
}

/// Pre-War Formalwear — {2}{W} Artifact — Equipment.
/// "When this Equipment enters, return target creature card with mana value 3 or less
///  from your graveyard to the battlefield and attach this Equipment to it."
/// "Equipped creature gets +2/+2 and has vigilance."
/// "Equip {3}"
fn pre_war_formalwear() -> CardDef {
    use crate::ir::ce::CEMod;
    use crate::ir::expr::Expr;
    let mut card = CardDef::new(
        "Pre-War Formalwear",
        CardKind::Artifact(ArtifactData {
            mana_cost: "2W".to_string(),
            subtypes: vec!["Equipment".into()],
            abilities: vec![AbilityDef {
                // Equip {3} — sorcery-speed, targets a creature you control.
                costs: ir_pay_mana_str("3"),
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Actor,
                    zone: ZoneId::Battlefield,
                    filter: ir_type(CardType::Creature),
                },
                ability_factory: Some(Arc::new(|who, source_id| {
                    Effect(Arc::new(move |state, _t, targets| {
                        let Some(&creature_id) = targets.first() else { return };
                        do_attach(state, who, source_id, creature_id);
                    }))
                })),
                timing: ActivationTiming::Sorcery,
                ..Default::default()
            }],
            mana_abilities: vec![],
        }),
        parse_colors("2W", false, false), None,
        vec![], CardLayout::Normal, None,
        // ETB: reanimate a creature in own GY with MV ≤ 3, then attach self to it.
        vec![TriggerDef {
            check: Arc::new(|event, source_id, controller, _state, pending| {
                if let GameEvent::ZoneChange { id, to: ZoneId::Battlefield, controller: ctlr, .. } = event {
                    if *id == source_id && *ctlr == controller {
                        pending.push(TriggerContext {
                            source_name: "Pre-War Formalwear".into(),
                            controller,
                            target_spec: TargetSpec::ObjectInZone {
                                controller: Who::Actor,
                                zone: ZoneId::Graveyard,
                                filter: ir_and(
                                    ir_type(CardType::Creature),
                                    ir_mv_le(3),
                                ),
                            },
                            effect: Effect(Arc::new(move |state, t, targets| {
                                let Some(&target_id) = targets.first() else { return };
                                change_zone(target_id, ZoneId::Battlefield, state, t, controller);
                                do_attach(state, controller, source_id, target_id);
                            })),
                        });
                    }
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![],
        vec![], // static CE now IR (card.abilities below)
    );
    // Equipped creature gets +2/+2 and has vigilance.
    card.abilities = vec![equipped_creature_ce(
        vec![
            CEMod::PumpPT(Expr::Num(2), Expr::Num(2)),
            CEMod::AddKeyword(Keyword::Vigilance),
        ],
        "Equipped creature gets +2/+2 and has vigilance.",
    )];
    card
}

/// Cryptic Coat — {2}{U} Artifact — Equipment.
/// "When this Equipment enters, cloak the top card of your library, then attach this
///  Equipment to it. (To cloak a card, put it onto the battlefield face down as a 2/2
///  creature with ward {2}. Turn it face up any time for its mana cost if it's a
///  creature card.)"
/// "Equipped creature gets +1/+0 and can't be blocked."
/// "{1}{U}: Return this Equipment to its owner's hand."
///
/// ABNORMAL simplifications:
///   * Cloak is modeled by creating a "Mysterious Creature" 2/2 token (no ward, no
///     turn-face-up, not actually the top card of the library). This matches how we
///     approximate Living Weapon via phyrexian_germ_token.
///   * "Can't be blocked" is not a supported keyword on the engine yet — we grant no
///     evasion, so combat interactions with equipped creatures are incorrect.
fn cryptic_coat() -> CardDef {
    use crate::ir::ce::CEMod;
    use crate::ir::expr::Expr;
    let mut card = CardDef::new(
        "Cryptic Coat",
        CardKind::Artifact(ArtifactData {
            mana_cost: "2U".to_string(),
            subtypes: vec!["Equipment".into()],
            abilities: vec![
                // {1}{U}: Return this Equipment to its owner's hand.
                AbilityDef {
                    costs: ir_pay_mana_str("1U"),
                    ability_factory: Some(Arc::new(|who, source_id| {
                        Effect(Arc::new(move |state, t, _targets| {
                            let owner = state.objects.get(&source_id).map(|o| o.owner).unwrap_or(who);
                            change_zone(source_id, ZoneId::Hand, state, t, owner);
                            state.log(t, who, "Cryptic Coat → bounced to hand".to_string());
                        }))
                    })),
                    ..Default::default()
                },
            ],
            mana_abilities: vec![],
        }),
        parse_colors("U", false, false), None,
        vec![], CardLayout::Normal, None,
        // ETB: cloak the top card of your library (ABNORMAL: token), then attach self to it.
        vec![TriggerDef {
            check: Arc::new(|event, source_id, controller, _state, pending| {
                if let GameEvent::ZoneChange { id, to: ZoneId::Battlefield, controller: ctlr, .. } = event {
                    if *id == source_id && *ctlr == controller {
                        pending.push(TriggerContext {
                            source_name: "Cryptic Coat".into(),
                            controller,
                            target_spec: TargetSpec::None,
                            effect: Effect(Arc::new(move |state, t, _targets| {
                                let token_id = do_create_token("Mysterious Creature", controller, state, t);
                                do_attach(state, controller, source_id, token_id);
                            })),
                        });
                    }
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![],
        vec![], // static CE now IR (card.abilities below)
    );
    // Equipped creature gets +1/+0. ("can't be blocked" omitted — no keyword.)
    card.abilities = vec![equipped_creature_ce(
        vec![CEMod::PumpPT(Expr::Num(1), Expr::Num(0))],
        "Equipped creature gets +1/+0.",
    )];
    card
}

/// Dragon's Rage Channeler — {R} 1/1 Human Shaman.
/// "Whenever you cast a noncreature spell, surveil 1."
/// "Delirium — As long as there are four or more card types among cards in your graveyard,
///  this creature gets +2/+2, has flying, and attacks each combat if able."
fn dragons_rage_channeler() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind};
    use crate::ir::ce::CEMod;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel, ZoneSel};
    let data = CreatureData::new("R", 1, 1);

    let mut card = CardDef::new(
        "Dragon's Rage Channeler",
        CardKind::Creature(data),
        parse_colors("R", false, false),
        None,
        vec![], CardLayout::Normal, None,
        // Trigger: "Whenever you cast a noncreature spell, surveil 1."
        vec![TriggerDef { check: Arc::new(|event, _source_id, controller, state, pending| {
            if let GameEvent::SpellCast { caster, card_id, .. } = event {
                if *caster != controller { return; }
                let is_creature = state.objects.get(card_id)
                    .and_then(|o| state.catalog.get(&o.catalog_key))
                    .map_or(false, |d| d.types.contains(&CardType::Creature));
                if !is_creature {
                    pending.push(TriggerContext {
                        source_name: "Dragon's Rage Channeler".into(),
                        controller,
                        target_spec: TargetSpec::None,
                        effect: eff_surveil(controller, 1),
                    });
                }
            }
        }), active_when: tp_on_battlefield() }],
        vec![],  // no replacements
        vec![],  // no prohibitions
        vec![],  // delirium CE now IR (card.abilities below)
    );
    // Delirium — as long as ≥4 card types among cards in your graveyard, DRC gets
    // +2/+2 and flying. One gated Static block: the delirium condition is a single
    // first-class gate (Decision 4) shared by both mods; scope is self.
    // ("attacks each combat if able" is not modeled.)
    let delirium = Expr::Ge(
        Box::new(Expr::Count(Box::new(Expr::Types(Box::new(Expr::AllObjects {
            zone: ZoneSel::Scoped {
                zone_kind: ZoneKindSel::Graveyard,
                owner: Box::new(Expr::Ctx(Ctx::Controller)),
            },
            bind: "g",
            filter: Box::new(Expr::Bool(true)),
        }))))),
        Box::new(Expr::Num(4)),
    );
    card.abilities = vec![Ability {
        kind: AbilityKind::Static {
            mods: vec![
                CEMod::PumpPT(Expr::Num(2), Expr::Num(2)),
                CEMod::AddKeyword(Keyword::Flying),
            ],
            scope: Some(Filter(Expr::Eq(
                Box::new(Expr::Ctx(Ctx::It)),
                Box::new(Expr::Ctx(Ctx::Source)),
            ))),
            condition: Some(delirium),
        },
        text: Some("Delirium — as long as there are four or more card types among cards in your graveyard, this creature gets +2/+2 and has flying."),
    }];
    card
}

/// Creature — Ape Spirit, 2/2. {2}{R}.
/// "Exile this card from your hand: Add {R}." — hand-zone mana ability (CR 605.3).
fn simian_spirit_guide() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, CostBody};
    use crate::ir::action::{Action, ManaSpec, Who};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let data = CreatureData::new("2R", 2, 2);
    let mut def = simple("Simian Spirit Guide", CardKind::Creature(data), parse_colors("R", false, false), None);
    def.abilities.push(Ability {
        kind: AbilityKind::Activated {
            // `ExileSelf` from hand: same source-self pattern as Street
            // Wraith's cycling cost — `Action::Move` (deterministic) rather
            // than `MoveByChoice`, since the activation pipeline pre-moves
            // hand-source abilities to Stack before paying costs.
            cost: CostBody::Ir(Action::Move {
                what: Expr::Ctx(Ctx::Source),
                to: ZoneKindSel::Exile,
                to_owner: None,
                bind_as: None,
            }),
            target_spec: TargetSpec::None,
            choice_spec: None,
            body: Action::AddMana {
                who: Who::You,
                count: Expr::Num(1),
                spec: ManaSpec::Fixed(vec![Color::Red]),
            },
            timing: ActivationTiming::Default,
            activation_condition: None,
            active_zone: ZoneKindSel::Hand,
        },
        text: Some("Exile Simian Spirit Guide from your hand: Add {R}."),
    });
    def
}

/// Fury — {3}{R}{R} Elemental Incarnation, 3/3. Double strike.
/// ETB: deals 4 damage divided as you choose among any number of target creatures
/// and/or planeswalkers. Evoke — Exile a red card from your hand. CR 702.74, 702.4.
fn fury() -> CardDef {
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let mut data = CreatureData::new("3RR", 3, 3);
    data.keywords = Keywords::from_slice(&[Keyword::DoubleStrike]);
    let mut c = CardDef::new(
        "Fury",
        CardKind::Creature(data),
        parse_colors("3RR", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![
            // ETB: deal 4 damage to target creature or planeswalker.
            etb_self_trigger("Fury", TargetSpec::ObjectInZone {
                controller: Who::Opp,
                zone: ZoneId::Battlefield,
                filter: ir_or(
                    ir_type(CardType::Creature),
                    ir_type(CardType::Planeswalker),
                ),
            }, |source_id, controller| eff_ir_targeted(controller, source_id, Action::DealDamage {
                source: Expr::Ctx(Ctx::Source),
                target: Expr::Ctx(Ctx::Var("target")),
                amount: Expr::Num(4),
            })),
            // Evoke sacrifice: if an alternate cost was used, sacrifice on ETB (CR 702.74).
            TriggerDef {
                check: Arc::new(|event, source_id, controller, state, pending| {
                    if let GameEvent::ZoneChange { id, to: ZoneId::Battlefield, controller: ctlr, .. } = event {
                        if *id == source_id && *ctlr == controller
                            && state.resolving_costs_ctx.alt_cost_index.is_some()
                        {
                            let sac_pred = ir_obj(source_id);
                            pending.push(TriggerContext {
                                source_name: "Fury (evoke)".into(),
                                controller,
                                target_spec: TargetSpec::None,
                                effect: eff_sacrifice(controller, Who::Actor, sac_pred),
                            });
                        }
                    }
                }),
                active_when: tp_on_battlefield(),
            },
        ],
        vec![],  // no replacements
        vec![],  // no statics
        vec![],  // no prohibitions
    );
    c.alternate_costs = vec![
        AlternateCost {
            // Evoke — Exile a red card from your hand.
            costs: CostBody::Ir(crate::ir::action::Action::MoveByChoice {
                who: crate::ir::action::Who::You,
                from: crate::ir::expr::ZoneKindSel::Hand,
                to: crate::ir::expr::ZoneKindSel::Exile,
                verb: crate::ir::action::MoveVerb::Exile,
                filter: pitch_color_filter(Color::Red),
                count: crate::ir::expr::Expr::Num(1),
                bind_as: Some("$fury_evoke"),
            }),
            hand_min: 2,
            ..Default::default()
        },
    ];
    c
}

/// Quantum Riddler — {3}{U}{U} Creature — Sphinx 4/6.
/// Flying.
/// "When this creature enters, draw a card."
/// "As long as you have one or fewer cards in hand, if you would draw one or more cards,
///  you draw that many cards plus one instead." — TODO: not modeled. Rulings (2025-07-25)
///  say the replacement applies at the draw-instruction level; the engine fires `Draw`
///  per card, so hooking it accurately requires a new `DrawInstruction` event.
/// "Warp {1}{U}" (CR 702.185): alternative cost; when cast for warp cost, a delayed
/// trigger at the beginning of the next end step exiles the permanent. TODO: the
/// "its owner may cast this card after the current turn has ended" part is not modeled
/// (requires a castable-from-exile flag tied to the exiled card).
fn quantum_riddler() -> CardDef {
    let mut data = CreatureData::new("3UU", 4, 6);
    data.creature_subtypes = vec!["Sphinx".into()];
    data.keywords = Keywords::from_slice(&[Keyword::Flying]);
    let mut c = CardDef::new(
        "Quantum Riddler",
        CardKind::Creature(data),
        parse_colors("3UU", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![
            // ETB: draw a card.
            etb_self_trigger("Quantum Riddler", TargetSpec::None,
                |_source_id, controller| eff_draw(controller, 1)),
            // Warp: if warp (alt) cost was used, register a delayed end-step exile.
            TriggerDef {
                check: Arc::new(|event, source_id, controller, state, pending| {
                    if let GameEvent::ZoneChange { id, to: ZoneId::Battlefield, controller: ctlr, .. } = event {
                        if *id == source_id && *ctlr == controller
                            && state.resolving_costs_ctx.alt_cost_index.is_some()
                        {
                            pending.push(TriggerContext {
                                source_name: "Quantum Riddler (warp)".into(),
                                controller,
                                target_spec: TargetSpec::None,
                                effect: Effect(Arc::new(move |state, _t, _targets| {
                                    state.trigger_instances.push(TriggerInstance {
                                        source_id,
                                        controller,
                                        check: Arc::new(move |event, _source_id, controller, _state, pending| {
                                            if let GameEvent::EnteredStep { step: StepKind::End, .. } = event {
                                                pending.push(TriggerContext {
                                                    source_name: "Quantum Riddler (warp exile)".into(),
                                                    controller,
                                                    target_spec: TargetSpec::None,
                                                    effect: Effect(Arc::new(move |state, t, _targets| {
                                                        change_zone(source_id, ZoneId::Exile, state, t, controller);
                                                    })),
                                                });
                                            }
                                        }),
                                        expiry: Some(Expiry::OneShot),
                                    });
                                })),
                            });
                        }
                    }
                }),
                active_when: tp_on_battlefield(),
            },
        ],
        vec![], vec![], vec![],
    );
    c.alternate_costs = vec![
        AlternateCost {
            costs: ir_pay_mana_str("1U"),
            ..Default::default()
        },
    ];
    c
}

/// Griselbrand — {4}{B}{B}{B}{B} Legendary 7/7 Demon.
/// Flying, lifelink. Pay 7 life: Draw seven cards.
fn griselbrand() -> CardDef {
    let mut data = CreatureData::new("4BBBB", 7, 7);
    data.legendary = true;
    data.keywords = Keywords::from_slice(&[Keyword::Flying, Keyword::Lifelink]);
    data.abilities = vec![AbilityDef {
        costs: ir_pay_life(7),
        ability_factory: Some(Arc::new(|who, _| eff_draw(who, 7))),
        ..Default::default()
    }];
    simple("Griselbrand", CardKind::Creature(data), parse_colors("4BBBB", false, false), None)
}

/// Emrakul, the Aeons Torn — {15} Legendary 15/15 Eldrazi.
/// Flying, annihilator 6, protection from spells that are one or more colors.
/// This spell can't be countered.
/// When you cast this spell, take an extra turn after this one.
/// When put into a graveyard from anywhere, owner shuffles graveyard into library.
/// TODO: cast trigger (extra turn), annihilator 6, graveyard shuffle not modeled.
/// Ward (CR 702.21) as a reusable IR triggered ability: when an opponent's spell
/// targets the holder, counter it unless its controller pays `cost`. The trigger
/// self-scopes via `Contains(Source, ChosenTargets(triggered_obj))`, so it works
/// both directly (self-ward) and when granted to another permanent via
/// `CEMod::GrantAbility` (then `Source` is the grantee). The pay-or-counter
/// decision runs in `Action::Ward` → `ward_pay_or_counter`.
// Transient: exercised by tests now; its first card consumer (Hexing Squelcher)
// lands in the immediately-following commit, which drops this allow.
#[allow(dead_code)]
pub(crate) fn ir_ward(cost: crate::ir::action::Action) -> crate::ir::ability::Ability {
    use crate::ir::ability::{Ability, AbilityKind, EventPattern, TriggerSpec};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel};
    Ability {
        kind: AbilityKind::Triggered {
            spec: TriggerSpec::When {
                pattern: EventPattern::SpellCast { spell_filter: Filter(Expr::Bool(true)) },
                condition: Some(Expr::And(
                    // cast by an opponent of the holder
                    Box::new(Expr::Not(Box::new(Expr::Eq(
                        Box::new(Expr::Ctx(Ctx::Var("triggered_actor"))),
                        Box::new(Expr::Ctx(Ctx::Controller)),
                    )))),
                    // targeting the holder: Source ∈ the spell's chosen targets
                    Box::new(Expr::Contains(
                        Box::new(Expr::Ctx(Ctx::Source)),
                        Box::new(Expr::ChosenTargets(Box::new(Expr::Ctx(Ctx::Var("triggered_obj"))))),
                    )),
                )),
            },
            target_spec: TargetSpec::None,
            body: Action::Ward { cost: Box::new(cost) },
            active_zone: ZoneKindSel::Battlefield,
        },
        text: Some("Ward (Whenever this permanent becomes the target of a spell an opponent controls, counter that spell unless its controller pays the ward cost.)"),
    }
}

/// "This spell can't be countered." An `AbilityKind::Prohibition` that matches a
/// `SpellBeingCountered` event whose spell is this source (`It == Source`) and
/// suppresses it in the event pipeline (CR 701.5 / 101.2 "can't beats can"). The
/// self-scoping pattern is its own armed-when gate: it only matches while the
/// source is the spell on the stack being countered.
fn cant_be_countered_self() -> crate::ir::ability::Ability {
    use crate::ir::ability::{Ability, AbilityKind, EventPattern};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};
    Ability {
        kind: AbilityKind::Prohibition {
            matches: EventPattern::SpellBeingCountered {
                spell_filter: Filter(Expr::Eq(
                    Box::new(Expr::Ctx(Ctx::It)),
                    Box::new(Expr::Ctx(Ctx::Source)),
                )),
            },
            // No zone gate: `It == Source` already pins this to the one spell on the
            // stack being countered (`counter_one` only fires on stack items).
            active_zone: None,
        },
        text: Some("This spell can't be countered."),
    }
}

fn emrakul_the_aeons_torn() -> CardDef {
    let mut data = CreatureData::new("15", 15, 15);
    data.legendary = true;
    data.keywords = Keywords::from_slice(&[Keyword::Flying, Keyword::Annihilator6]);
    let mut def = CardDef::new(
        "Emrakul, the Aeons Torn",
        CardKind::Creature(data),
        vec![],  // colorless
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![],
        vec![], // "can't be countered" is now an IR Prohibition (def.abilities below)
        vec![],
    );
    def.counterable = false;
    def.protection_from = vec![ir_colored_spell()];
    def.abilities.push(cant_be_countered_self());
    def
}

/// Atraxa, Grand Unifier — {3}{G}{W}{U}{B} Legendary 7/7 Phyrexian Angel.
/// Flying, vigilance, deathtouch, lifelink.
/// ETB: reveal top 10 of library, for each card type you may put one into hand, rest to bottom.
/// TODO: real ETB needs per-type strategy choices over actual revealed cards; placeholder
/// adds 4 cards to hand silently (no Draw events).
fn atraxa_grand_unifier() -> CardDef {
    let mut data = CreatureData::new("3GWUB", 7, 7);
    data.legendary = true;
    data.keywords = Keywords::from_slice(&[
        Keyword::Flying, Keyword::Vigilance, Keyword::Deathtouch, Keyword::Lifelink,
    ]);
    CardDef::new(
        "Atraxa, Grand Unifier",
        CardKind::Creature(data),
        parse_colors("3GWUB", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![TriggerDef { check: Arc::new(atraxa_etb_check), active_when: tp_on_battlefield() }],
        vec![],
        vec![],
        vec![],
    )
}

fn brazen_borrower() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel};
    let mut back = simple(
        "Petty Theft",
        CardKind::Instant(SpellData {
            mana_cost: "1U".to_string(),
            subtypes: vec!["adventure".to_string()],
            modes: None,
            ..Default::default()
        }),
        parse_colors("1UU", true, false),
        None,
    );
    back.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Battlefield,
                    filter: ir_not(ir_type(CardType::Land)),
                },
                body: Action::Return {
                    what: Expr::Ctx(Ctx::Var("target")),
                    to: ZoneKindSel::Hand,
                    bind_as: None,
                },
            }],
        },
        text: Some("Return target nonland permanent to its owner's hand."),
    }];

    let mut data = CreatureData::new("1UU", 3, 1);
    data.keywords.insert(Keyword::Flash);
    data.keywords.insert(Keyword::Flying);

    CardDef::new(
        "Brazen Borrower",
        CardKind::Creature(data),
        parse_colors("1UU", true, false),
        None,
        vec![], CardLayout::Split, Some(Box::new(back)),
        vec![], vec![], vec![], vec![],
    )
}

/// Mishra's Bauble — {0} Artifact.
/// {T}, Sacrifice: Look at the top card of target player's library.
/// Draw a card at the beginning of the next turn's upkeep.
fn mishras_bauble() -> CardDef {
    simple("Mishra's Bauble", CardKind::Artifact(ArtifactData {
        mana_cost: "0".to_string(),
        abilities: vec![AbilityDef {
            costs: ir_seq(vec![act_tap_source(), act_sac_self("$bauble_self")]),
            ability_factory: Some(Arc::new(|who, _source_id| {
                Effect(Arc::new(move |state, t, _targets| {
                    // "Look at the top card" — informational only in the sim.
                    // Delayed trigger: draw a card at the beginning of the next upkeep.
                    state.trigger_instances.push(TriggerInstance {
                        source_id: ObjId::UNSET,
                        controller: who,
                        check: Arc::new(move |event, _source_id, controller, _state, pending| {
                            if let GameEvent::EnteredStep { step: StepKind::Upkeep, .. } = event {
                                pending.push(TriggerContext {
                                    source_name: "Mishra's Bauble (delayed draw)".into(),
                                    controller,
                                    target_spec: TargetSpec::None,
                                    effect: eff_draw(controller, 1),
                                });
                            }
                        }),
                        expiry: Some(Expiry::OneShot),
                    });
                    state.log(t, who, "Mishra's Bauble → draw at next upkeep".to_string());
                }))
            })),
            ..Default::default()
        }],
        ..Default::default()
    }), vec![], Some(25))
}

/// Containment Priest — {1}{W} Creature — Human Cleric 2/2. Flash.
/// If a nontoken creature would enter the battlefield and it wasn't cast,
/// exile it instead.
fn containment_priest() -> CardDef {
    let mut data = CreatureData::new("1W", 2, 2);
    data.keywords.insert(Keyword::Flash);
    CardDef::new(
        "Containment Priest",
        CardKind::Creature(data),
        parse_colors("1W", true, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![],  // no triggers
        // Replacement: nontoken creature entering BF from non-Stack → exile instead.
        vec![ReplacementDef {
            check: Arc::new(|event, source_id, _controller, state| {
                if let GameEvent::ZoneChange { id, from, to: ZoneId::Battlefield, .. } = event {
                    // "wasn't cast" = not entering from the stack
                    if *from == ZoneId::Stack { return None; }
                    // nontoken
                    let obj = state.objects.get(id)?;
                    if obj.is_token { return None; }
                    // creature
                    let def = state.catalog.get(&obj.catalog_key)?;
                    if !def.is_creature() { return None; }
                    // don't exile itself entering via non-cast
                    if *id == source_id { return None; }
                    Some(vec![*id])
                } else {
                    None
                }
            }),
            make_effect: Arc::new(|_source_id, controller: PlayerId| {
                Effect(Arc::new(move |state, t, targets| {
                    if let Some(&id) = targets.first() {
                        change_zone(id, ZoneId::Exile, state, t, controller);
                    }
                }))
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![],  // no prohibitions
        vec![],  // no static abilities
    )
}

// ── Delver of Secrets ────────────────────────────────────────────────────────

/// Delver of Secrets — {U} Creature — Human Wizard 1/1. DFC.
/// "At the beginning of your upkeep, look at the top card of your library.
///  You may reveal that card. If an instant or sorcery card is revealed this way,
///  transform this creature."
/// Back face: Insectile Aberration — 3/2 Flying.
fn delver_of_secrets() -> CardDef {
    let back = CardDef::new(
        "Insectile Aberration",
        CardKind::Creature({
            let mut c = CreatureData::new("", 3, 2);
            c.keywords = Keywords::from_slice(&[Keyword::Flying]);
            c
        }),
        parse_colors("U", false, false),
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![], vec![], vec![],
    );

    CardDef::new(
        "Delver of Secrets",
        CardKind::Creature(CreatureData::new("U", 1, 1)),
        parse_colors("U", false, false),
        Some(50),
        vec![], CardLayout::DoubleFaced, Some(Box::new(back)),
        // Upkeep trigger: look at top card, if instant/sorcery, transform.
        vec![TriggerDef {
            check: Arc::new(move |event, source_id, controller, state, pending| {
                if let GameEvent::EnteredStep { step: StepKind::Upkeep, active_player } = event {
                    if *active_player != controller { return; }
                    // Only flip from front face.
                    if state.permanent_bf(source_id).map_or(true, |bf| bf.active_face != 0) { return; }
                    // Check top card of library.
                    let is_instant_or_sorcery = state.library_of(controller).next()
                        .and_then(|obj| state.catalog.get(&obj.catalog_key))
                        .map_or(false, |d| d.is_instant() || d.is_sorcery());
                    if !is_instant_or_sorcery { return; }
                    pending.push(TriggerContext {
                        source_name: "Delver of Secrets".into(),
                        controller,
                        target_spec: TargetSpec::None,
                        // "Transform this creature" — literal in-place flip (CR 701.28).
                        effect: eff_ir(controller, crate::ir::action::Action::Transform {
                            target: crate::ir::expr::Expr::ObjLit(source_id),
                        }),
                    });
                }
            }),
            active_when: tp_on_battlefield(),
        }],
        vec![], vec![], vec![],
    )
}

// ── Unholy Heat ──────────────────────────────────────────────────────────────

/// Unholy Heat — {R} Instant. Deals 2 damage to target creature or planeswalker.
/// Delirium — deals 6 damage instead if ≥4 card types in graveyard.
fn unholy_heat() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, ZoneKindSel, ZoneSel};

    // Delirium: four or more card types among cards in your graveyard
    // (CR 700.5) — `Count` of the deduped type-union over your graveyard.
    let delirium = Expr::Ge(
        Box::new(Expr::Count(Box::new(Expr::Types(Box::new(Expr::AllObjects {
            zone: ZoneSel::Scoped {
                zone_kind: ZoneKindSel::Graveyard,
                owner: Box::new(Expr::Ctx(Ctx::Controller)),
            },
            bind: "g",
            filter: Box::new(Expr::Bool(true)),
        }))))),
        Box::new(Expr::Num(4)),
    );
    let damage = |n: i64| Action::DealDamage {
        source: Expr::Ctx(Ctx::Source),
        target: Expr::Ctx(Ctx::Var("target")),
        amount: Expr::Num(n),
    };

    let mut card = simple("Unholy Heat", CardKind::Instant(SpellData {
        mana_cost: "R".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("R", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::Union(vec![
                    TargetSpec::ObjectInZone { controller: Who::Opp, zone: ZoneId::Battlefield, filter: ir_type(CardType::Creature) },
                    TargetSpec::ObjectInZone { controller: Who::Opp, zone: ZoneId::Battlefield, filter: ir_type(CardType::Planeswalker) },
                ]),
                body: Action::IfThen {
                    cond: delirium,
                    then: Box::new(damage(6)),
                    else_: Some(Box::new(damage(2))),
                },
            }],
        },
        text: Some("Unholy Heat deals 2 damage to target creature or planeswalker. Delirium — 6 damage instead if there are four or more card types among cards in your graveyard."),
    }];
    card
}

// ── Price of Progress ────────────────────────────────────────────────────────

/// Price of Progress — {1}{R} Instant. Deals damage to each player equal to
/// twice the number of nonbasic lands that player controls.
fn price_of_progress() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter, ZoneKindSel, ZoneSel};

    // Nonbasic lands the bound player `p` controls (`p` is a player object).
    let nonbasic_lands_controlled = || Expr::AllObjects {
        zone: ZoneSel::Global(ZoneKindSel::Battlefield),
        bind: "it",
        filter: Box::new(ir_and(
            ir_and(ir_type(CardType::Land), ir_not(ir_supertype(Supertype::Basic))),
            Filter(Expr::Eq(
                Box::new(Expr::Controller(Box::new(Expr::Ctx(Ctx::It)))),
                Box::new(Expr::Ctx(Ctx::Var("p"))),
            )),
        ).0),
    };

    let mut card = simple("Price of Progress", CardKind::Instant(SpellData {
        mana_cost: "1R".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("R", false, false), None);
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                // The direct oracle form: for each player, deal 2 × the nonbasic
                // lands they control. Players are objects, so this is a ForEach
                // over `Expr::Players`; damage to a player object is life loss.
                body: Action::ForEach {
                    over: Expr::Players,
                    bind: "p",
                    body: Box::new(Action::DealDamage {
                        source: Expr::Ctx(Ctx::Source),
                        target: Expr::Ctx(Ctx::Var("p")),
                        amount: Expr::Mul(
                            Box::new(Expr::Num(2)),
                            Box::new(Expr::Count(Box::new(nonbasic_lands_controlled()))),
                        ),
                    }),
                },
            }],
        },
        text: Some("Price of Progress deals damage to each player equal to twice the number of nonbasic lands they control."),
    }];
    card
}

// ── Meltdown ─────────────────────────────────────────────────────────────────

/// Meltdown — {X}{R} Sorcery. "Destroy each artifact with mana value X or less."
fn meltdown() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;

    let mut def = simple("Meltdown", CardKind::Sorcery(SpellData {
        mana_cost: "R".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("R", false, false), None);
    def.additional_costs = ir_xmana_cost();
    def.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                // Destroy each artifact whose mana value is ≤ the announced X
                // (bound as `Ctx::Var("x")` by `build_spell_effect`).
                body: ir_for_each_on_battlefield(
                    ir_and(ir_type(CardType::Artifact), ir_mv_le_expr(Expr::Ctx(Ctx::Var("x")))),
                    Action::Destroy { target: Expr::Ctx(Ctx::Var("v")) },
                ),
            }],
        },
        text: Some("Destroy each artifact with mana value X or less."),
    }];
    def
}

// ── Rough // Tumble ──────────────────────────────────────────────────────────

/// Rough // Tumble — split card (first true split, not adventure).
/// Rough: {1}{R} Sorcery — "Rough deals 2 damage to each creature without flying."
/// Tumble: {5}{R} Sorcery — "Tumble deals 6 damage to each creature with flying."
fn rough_tumble() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};

    // "deals `n` damage to each creature matching `flying_filter`". Protection is
    // enforced by the `DealDamage` primitive (CR 702.16e), so no per-card check.
    let damage_each = |n: i64, flying_filter: Filter| ir_for_each_on_battlefield(
        ir_and(ir_type(CardType::Creature), flying_filter),
        Action::DealDamage {
            source: Expr::Ctx(Ctx::Source),
            target: Expr::Ctx(Ctx::Var("v")),
            amount: Expr::Num(n),
        },
    );

    let mut tumble = simple("Tumble", CardKind::Sorcery(SpellData {
        mana_cost: "5R".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("R", false, false), None);
    tumble.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: damage_each(6, ir_keyword(Keyword::Flying)),
            }],
        },
        text: Some("Tumble deals 6 damage to each creature with flying."),
    }];

    let mut card = CardDef::new(
        "Rough // Tumble",
        CardKind::Sorcery(SpellData {
            mana_cost: "1R".to_string(),
            modes: None,
            ..Default::default()
        }),
        parse_colors("R", false, false),
        None,
        vec![], CardLayout::Split, Some(Box::new(tumble)),
        vec![], vec![], vec![], vec![],
    );
    card.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::None,
                body: damage_each(2, ir_not(ir_keyword(Keyword::Flying))),
            }],
        },
        text: Some("Rough deals 2 damage to each creature without flying."),
    }];
    card
}

// ── Prismatic Ending ─────────────────────────────────────────────────────────

/// Prismatic Ending — {X}{W} Sorcery.
/// Converge — Exile target nonland permanent if its mana value is less than or
/// equal to the number of colors of mana spent to cast this spell.
///
/// Modeled as base cost {W} plus `XMana` additional cost (same sunburst pattern
/// as Engineered Explosives / Meltdown — strategy declares `chosen_x` distinct
/// colored mana toward the {X} generic). Converge count = chosen_x + 1; the +1
/// is the mandatory {W} pip. At resolution, the target is exiled iff its mana
/// value ≤ converge count; otherwise the effect does nothing (CR 702.103a).
fn prismatic_ending() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, IrSpellMode};
    use crate::ir::action::Action;
    use crate::ir::context::Ctx;
    use crate::ir::expr::Expr;
    let mut def = simple("Prismatic Ending", CardKind::Sorcery(SpellData {
        mana_cost: "W".to_string(),
        modes: None,
        ..Default::default()
    }), parse_colors("W", false, false), None);
    // Converge = number of colors of mana spent, modeled as the announced X + 1
    // (`Ctx::Var("x")` bound by `build_spell_effect`). Exile the target iff its
    // mana value ≤ converge; otherwise the spell does nothing.
    def.abilities = vec![Ability {
        kind: AbilityKind::OnResolve {
            modes: vec![IrSpellMode {
                target_spec: TargetSpec::ObjectInZone {
                    controller: Who::Opp,
                    zone: ZoneId::Battlefield,
                    filter: ir_not(ir_type(CardType::Land)),
                },
                body: Action::IfThen {
                    cond: Expr::Le(
                        Box::new(Expr::Mv(Box::new(Expr::Ctx(Ctx::Var("target"))))),
                        Box::new(Expr::Add(
                            Box::new(Expr::Ctx(Ctx::Var("x"))),
                            Box::new(Expr::Num(1)),
                        )),
                    ),
                    then: Box::new(Action::Exile {
                        target: Expr::Ctx(Ctx::Var("target")),
                        bind_as: None,
                    }),
                    else_: None,
                },
            }],
        },
        text: Some("Exile target nonland permanent if its mana value is less than or equal to the amount of mana spent to cast this spell."),
    }];
    def.additional_costs = ir_xmana_cost();
    def
}

// ── Null Rod ─────────────────────────────────────────────────────────────────

/// Null Rod — {2} Artifact. "Activated abilities of artifacts can't be activated."
/// An action-Restriction (Activate) over artifacts — symmetric, and its subject has
/// no mana-ability exemption, so it covers mana abilities too (shuts off Moxen).
/// CR 101.2 "can't beats can".
fn null_rod() -> CardDef {
    use crate::ir::ability::{Ability, AbilityKind, ActionKind};
    use crate::ir::context::Ctx;
    use crate::ir::expr::{Expr, Filter};
    let mut card = CardDef::new(
        "Null Rod",
        CardKind::Artifact(ArtifactData {
            mana_cost: "2".to_string(),
            ..Default::default()
        }),
        vec![],
        None,
        vec![], CardLayout::Normal, None,
        vec![], vec![], vec![],
        vec![], // "can't activate" is now an AbilityKind::Restriction (card.abilities below)
    );
    // "Activated abilities of artifacts can't be activated." Symmetric (both players').
    // An action-Restriction consulted at activation-legality (incl. mana abilities) —
    // keyed on the *source* permanent: any artifact's activations are forbidden.
    card.abilities = vec![Ability {
        kind: AbilityKind::Restriction {
            action: ActionKind::Activate,
            subject: Filter(Expr::Contains(
                Box::new(Expr::TypeLit(CardType::Artifact)),
                Box::new(Expr::Types(Box::new(Expr::Ctx(Ctx::It)))),
            )),
        },
        text: Some("Activated abilities of artifacts can't be activated."),
    }];
    card
}
