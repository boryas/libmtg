//! `dd-goldfish` CLI: run the Doomsday goldfish simulator and render the result.
//! Goldfishes a deck loaded from a text file or a Moxfield/MTGGoldfish URL, or
//! a built-in sample deck when `--deck` is omitted.

use std::process::ExitCode;

use clap::Parser;
use dd_goldfish::{run_goldfish, sample_doomsday_deck, GoldfishStats, DEFAULT_PROTECTION};
use mtg_engine::{build_catalog, warn_unimplemented_cards};

#[derive(Parser)]
#[command(about = "Goldfish Monte-Carlo simulator for Doomsday (race to cast DD)")]
struct Args {
    /// Deck to goldfish: a text decklist file, or a Moxfield/MTGGoldfish URL.
    /// Omit to use the built-in sample Doomsday list.
    #[arg(long)]
    deck: Option<String>,
    /// Number of games to simulate.
    #[arg(long, default_value_t = 10_000)]
    games: u32,
    /// Turn cap per game.
    #[arg(long, default_value_t = 10)]
    max_turns: u8,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let deck = match &args.deck {
        Some(spec) => match load_deck(spec) {
            Ok(deck) => deck,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => sample_doomsday_deck(),
    };

    // Surface cards the engine can't simulate (dropped / inert) before running.
    warn_unimplemented_cards(&deck, "deck", &build_catalog());

    eprintln!(
        "Goldfishing {} games (cap {} turns)…",
        args.games, args.max_turns
    );
    let stats = run_goldfish(&deck, args.games, DEFAULT_PROTECTION, args.max_turns);
    print_report(&stats, args.max_turns);
    ExitCode::SUCCESS
}

/// Resolve a `--deck` argument (URL or file path) to the engine deck format.
fn load_deck(spec: &str) -> Result<Vec<(String, i32, String)>, String> {
    let deck = if spec.starts_with("http://") || spec.starts_with("https://") {
        decklist::from_url(spec).map_err(|e| e.to_string())?
    } else {
        let content = std::fs::read_to_string(spec)
            .map_err(|e| format!("reading deck file {spec}: {e}"))?;
        decklist::Decklist::parse_text(&content)
    };
    if deck.main.is_empty() {
        return Err(format!("no mainboard cards parsed from {spec}"));
    }
    Ok(deck.to_engine_deck())
}

fn bar(c: u32, max: u32, width: usize) -> String {
    let n = if max == 0 {
        0
    } else {
        (c as usize * width) / max as usize
    };
    "█".repeat(n)
}

fn print_report(s: &GoldfishStats, max_turns: u8) {
    println!("\n══ Doomsday goldfish — {} games ══", s.games);
    println!(
        "  cast DD:         {} ({:.1}%)",
        s.successes(),
        100.0 * (1.0 - s.fail_rate())
    );
    println!("  failed:          {} ({:.1}%)", s.fails, 100.0 * s.fail_rate());
    println!("  mean cast turn:  {:.2}", s.mean_cast_turn());
    println!("  mean protection: {:.2}", s.mean_protection());

    println!("\n  cast-turn distribution:");
    let max_c = s.cast_turn.values().copied().max().unwrap_or(1);
    for t in 1..=max_turns {
        let c = s.cast_turn.get(&t).copied().unwrap_or(0);
        println!(
            "    T{:<2} {:>7} {:>5.1}% │{}",
            t,
            c,
            100.0 * c as f64 / s.games.max(1) as f64,
            bar(c, max_c, 40)
        );
    }

    println!("\n  protection in hand at cast (of {} casts):", s.successes());
    let max_p = s.protection.values().copied().max().unwrap_or(1);
    let maxk = s.protection.keys().copied().max().unwrap_or(0);
    for k in 0..=maxk {
        let c = s.protection.get(&k).copied().unwrap_or(0);
        println!(
            "    {} {:>7} {:>5.1}% │{}",
            k,
            c,
            100.0 * c as f64 / s.successes().max(1) as f64,
            bar(c, max_p, 40)
        );
    }

    // Cumulative "cast by turn N" curve.
    use textplots::{Chart, Plot, Shape};
    let pts: Vec<(f32, f32)> = (1..=max_turns)
        .map(|t| (t as f32, (100.0 * s.cast_by(t)) as f32))
        .collect();
    println!("\n  P(cast by turn) %:");
    Chart::new(100, 50, 1.0, max_turns as f32)
        .lineplot(&Shape::Lines(&pts))
        .display();
}
