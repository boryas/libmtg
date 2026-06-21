# Example analysis: what goes in the flex slots?

The tempo doomsday list built by Eureka22422 that has been showing results recently can be found here: https://www.mtggoldfish.com/deck/7813548#paper. Notably, it contains 3 wastelands and 3 ponders. This "heresy" against ponder, especially with 4 flow state to fuel, had a lot of people wondering questions about the best use of the flex slots in the tempo doomsday shell. Since the printing of flow state, tempo doomsday plays a very lean package of both tempo and doomsday cards. There is the bare minimum of combo cards (2 wincons, 2 cyclers, 2 petals), and no more Orcish Bowmasters alongside Tamiyo and Murktide as the tempo threats. There are only 4 interaction spells (3 Thoughtseize, 1 Daze) to back up the 4 Force of Wills. So really, there just aren't that many flex slots left at all. One big question in the community was around the importance of wasteland, and whether the wastelands could be sacrificed to try to go faster. The two questions this analysis aims to address are: 1. should we play 3 ponder + 3 Wasteland or 4 Ponder + 2 Wasteland, and 2. Should we cut the wastelands altogether for more raw speed in the form of petals, street wraiths, personal tutors, etc.

The doomsday goldfish sim will help us understand how quickly can various configurations of the tempo build assemble a T3 doomsday when it decides to dig as hard as possible for the doomsday. This is meant to model a matchup like Boros Energy where we need to cast the Doomsday before the cats tear us to shreds. We will use the goldfish sim to build up our intution about the effect tweaking the numbers of the cards even has, and to try to directly compare the concrete proposed configurations.

---

## How to read these numbers

- **Metric:** `P(cast Doomsday by turn 3)` — the chance a solitaire game assembles and
  casts Doomsday by T3.
- **Mulligan mode:** all numbers below use `Realistic` — a player-faithful
  plan-coverage rule (keep a hand iff it has a viable plan to the combo). How the
  mulligan strategy *itself* changes the numbers is a separate question for another
  article.
- **All runs:** 5000 goldfish games on the one deck above (Eureka's list, +1 Ponder /
  −1 Wasteland to a 4-Ponder base for the sweeps).

---

## Question 1 — Ponder vs. Wasteland

Trade Ponder (premier cantrip) ↔ Wasteland (utility land), total held at 6.

| Build         | P(DD<=T3) |
|---------------|--------------|
| 2 Pon / 4 Wst | 59.8% |
| 3 Pon / 3 Wst | 58.7% |
| 4 Pon / 2 Wst | 60.2% |

- Per-card slope: **≈ flat** — all ~59–60%, the differences are within Monte-Carlo noise.

**Takeaway:** The marginal selection of the 4th ponder does not substantially improve the probability of a T3 doomsday, so that should not be considered the reason to prefer Ponder over Wasteland. It can, of course, still be better to play 4 Ponder for cantripping into lands, enabling Flow State, etc. Also, the Realistic mulligan strategy shows the strongest effects in general, so we will prefer to use it to differentiate builds going forward.

---

## Question 2 — Effect of more Lotus Petals

N Lotus Petal replacing cards that don't affect fast doomsdays.

| N       | P(DD<=T3) |
|---------|--------------|
| 0 Petal | 56.0%  |
| 1 Petal | 57.9%  |
| 2 Petal | 58.8% |
| 3 Petal | 62.3% |
| 4 Petal | 65.1% |

- Per-card slope: **≈ +2% / Petal**.

**Takeaway:** Marginal fast mana helps us much more than a Ponder to get to Doomsday+BBB before T3, and does meaningfully increase the speed of the deck.

---

## Question 3 — What about Personal Tutor?

Assuming we cut wasteland, I imagine we revert to playing 4 Ponder again (maybe that is a bad assumption?) so we consider the other 2 Wastelands the 2 flex slots. Compare Lotus Petal vs Personal Tutor in those slots:

The deck's 2 stock Petals are always kept; the table varies the **total** Petal and PT counts as the two ex-Wasteland slots are filled.

| Flex fill            | Total Petal / PT | P(DD<=T3) |
|----------------------|------------------|----------|
| 2 Wasteland (stock)  | 2 Petal / 0 PT   | 60.0%  |
| +2 Petal             | 4 Petal / 0 PT   | 63.9%  |
| +1 Petal +1 PT       | 3 Petal / 1 PT   | 66.3%  |
| +2 PT                | 2 Petal / 2 PT   | 68.5%  |

- Slopes here: **≈ +4% / PT** vs **≈ +2% / Petal** (still ~additive: 1/1 predicts 66.2, measures 66.3)

**Takeaway:** On pure speed Personal Tutor yields almost double the effect of Lotus Petal. This is predictable as the deck already has more mana (lands, rituals, and petals) than Doomsdays.

---

## Conclusion

If optimizing for straightline speed, Personal Tutor has the highest gain of the available options. The 4th Ponder has a marginal impact on pure speed. My personal conclusion is that Tempo Doomsday should give up the ~8% chance of a T3 DD to play Wasteland, which synergizes heavily with the decks tempo gameplans and pivots. It also has non-trivial application tapping for mana to crack clues, cast Flow States and Barrowgoyfs, etc.

## Method footnotes _(optional, for the curious)_

- The strategy employed in the simulations has a deterministic Doomsday line solver and can also predict a minimal time to Doomsday under perfect draws. When digging for Doomsday, it uses these to drive cantripping / shuffling / Personal Tutor decisions to attempt to put a Doomsday on the stack by the cutoff turn (in this case T3)
- The sim stops at the cutoff turn, so **Avg DD Turn** is the mean turn *among games that combo by T3* (hence it sits below 3, not a separate "eventual" speed).
- Jace, Wielder of Mysteries (the alt wincon) isn't modeled; it's treated as an inert blank card. It never helps cast Doomsday, so this only costs it its deck slot — which is the honest cost of a do-nothing-for-the-combo card.

- More information on the simulator, goldfish strategy, etc. can be found at bur.io/doomsday/goldfish and in the code repo at https://github.com/boryas/libmtg
