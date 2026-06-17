//! `dd-goldfish` CLI: run the Doomsday goldfish simulator and render the result.
//! Decklist/URL input lands later; for now it goldfishes a built-in sample deck.

use clap::Parser;
use dd_goldfish::{run_goldfish, sample_doomsday_deck, GoldfishStats, DEFAULT_PROTECTION};

#[derive(Parser)]
#[command(about = "Goldfish Monte-Carlo simulator for Doomsday (race to cast DD)")]
struct Args {
    /// Number of games to simulate.
    #[arg(long, default_value_t = 10_000)]
    games: u32,
    /// Turn cap per game.
    #[arg(long, default_value_t = 10)]
    max_turns: u8,
}

fn main() {
    let args = Args::parse();
    eprintln!(
        "Goldfishing {} games (cap {} turns)…",
        args.games, args.max_turns
    );
    let stats = run_goldfish(
        &sample_doomsday_deck(),
        args.games,
        DEFAULT_PROTECTION,
        args.max_turns,
    );
    print_report(&stats, args.max_turns);
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
