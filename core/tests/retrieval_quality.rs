//! Labelled retrieval eval against the gates in `PLAN.md` §8: top-1 ≥ 90%, top-3 ≥ 98%.
//!
//! # How the queries were written
//!
//! Five per card, ninety in total, written as the person holding the phone would type
//! them — half a sentence, no medical vocabulary, someone else's emergency in the room.
//! They were written by reading each card's `title`, `applies_to` and `also_called` and
//! asking "what would someone say who needs *this* card and has never read it", **not** by
//! running a query and keeping the ones that passed. That order matters: a suite tuned
//! until it goes green measures nothing except the tuner's patience. Where a query failed,
//! the rule was to fix the corpus or the ranker and leave the query alone.
//!
//! Every fix this suite forced is recorded where it landed, with the failing query in the
//! comment:
//!
//! - [`prohori_core::retrieval::is_search_noise`] gained articles, possessives,
//!   quantifiers, interrogatives, hedges, and continuation verbs — "I *think* he is having
//!   a heart attack" was returning the stroke card on the strength of "think", and "how
//!   much does a taxi cost" was returning three cards on "how" and "much".
//! - [`prohori_core::retrieval`] gained a phrase index over the declaration fields, because
//!   "cannot breathe" is two of the commonest words in the corpus and a bag of words has
//!   thrown away the only thing that made them mean something.
//! - `prohori_core::retrieval::Field::is_declaration` split "the card says this about
//!   itself" from "this appears somewhere in the card". Searching "choking" returned
//!   **nothing** before that: ten cards mention choking in a `do_not` line, so the word
//!   looked too common to be informative.
//! - `heat.illness`'s title, because the word "heat" was not in the field that carries
//!   weight.
//! - `poisoning.swallowed`'s search phrase "took too many" became "took too many pills".
//!   Every word of the original is a function word; the card was unfindable by the phrase
//!   it had chosen to advertise.
//!
//! # What the gates do and do not promise
//!
//! Top-1 ≥ 90% says the first card is usually right. It does not say the first card is
//! safe to act on blindly, and nothing here treats a hit as triage: a hit sets no severity,
//! bypasses no model, and never suppresses a red flag. That separation is
//! `docs/CONVENTIONS.md` §10, and the red-flag layer is tested on its own in
//! `redflag_safety.rs`. Retrieval failing means someone hunts for a card. The red-flag
//! layer failing means someone is told the wrong thing about a dying person. These are not
//! the same kind of test and the gates are deliberately different.
//! `the_rule_table_and_not_the_ranker_owns_the_critical_cases` states that division in
//! code.
//!
//! # Known residuals, stated rather than hidden
//!
//! - "my kid is choking" is weaker than "my child is choking". `normalize` folds
//!   spellings, never synonyms, so `kid` does not become `child`; a card that wants that
//!   word says so in its own `also_called`. See `docs/CONVENTIONS.md` §6.
//! - A query naming two cards ranks one of them first, and which one is a judgement the
//!   corpus makes, not something this suite can settle. `a_query_that_names_two_emergencies_offers_both`
//!   asserts only that neither disappears.
//! - Three of the 154 search phrases do not rank their own card first, though all 154 reach
//!   the top three. Two cards can honestly answer "vomiting" or "swallowed something", and
//!   the gate is deliberately on the top three for exactly that reason.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use prohori_core::bundled;
use prohori_core::retrieval::{Hit, Index};

fn index() -> Index {
    let (corpus, errors) = bundled::corpus();
    assert!(errors.is_empty(), "bundled corpus has errors: {errors:?}");
    Index::build(&corpus)
}

fn top_ids(hits: &[Hit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.protocol_id.as_str()).collect()
}

/// Labelled queries: what someone types, and the card they need.
///
/// Grouped by expected card so a reviewer can read five queries against one card and judge
/// whether they are honest. Nothing here is a phrase copied whole out of a card except
/// where a person would plausibly type exactly that ("heat stroke", "signs of a stroke").
const CASES: &[(&str, &str)] = &[
    // allergy.anaphylaxis
    (
        "she ate a peanut and her lips are swelling",
        "allergy.anaphylaxis",
    ),
    (
        "allergic reaction with a swollen throat",
        "allergy.anaphylaxis",
    ),
    ("where is the epipen", "allergy.anaphylaxis"),
    (
        "he was stung by a bee and came out in hives",
        "allergy.anaphylaxis",
    ),
    ("anaphylaxis what do i do", "allergy.anaphylaxis"),
    // bleeding.severe
    (
        "deep cut on his arm and blood everywhere",
        "bleeding.severe",
    ),
    ("the blood will not stop", "bleeding.severe"),
    ("he was stabbed in the leg", "bleeding.severe"),
    ("how do i put on a tourniquet", "bleeding.severe"),
    (
        "she is bleeding fast and soaking through the cloth",
        "bleeding.severe",
    ),
    // breathing.distress
    (
        "he is wheezing and struggling to breathe",
        "breathing.distress",
    ),
    ("asthma attack and no inhaler", "breathing.distress"),
    ("she is short of breath", "breathing.distress"),
    (
        "his lips are turning blue and he cannot get air",
        "breathing.distress",
    ),
    ("he needs his puffer", "breathing.distress"),
    // burn.thermal
    ("i burnt my hand on a pan", "burn.thermal"),
    ("the pan of oil splashed on me", "burn.thermal"),
    ("steam from the kettle got my arm", "burn.thermal"),
    ("should i put ice on a burn", "burn.thermal"),
    ("he has sunburn and blisters", "burn.thermal"),
    // chest.pain
    ("i think he is having a heart attack", "chest.pain"),
    ("crushing pain in my chest", "chest.pain"),
    ("his chest feels tight and heavy", "chest.pain"),
    ("pain going into my left arm", "chest.pain"),
    ("he has chest pain that will not go away", "chest.pain"),
    // choking.adult
    ("food is stuck in his throat", "choking.adult"),
    ("my child swallowed a coin", "choking.adult"),
    ("he cannot cough or speak", "choking.adult"),
    ("she is choking at the dinner table", "choking.adult"),
    ("how do i do the heimlich", "choking.adult"),
    // cpr.adult
    ("he is not breathing and has no pulse", "cpr.adult"),
    ("how do i do cpr", "cpr.adult"),
    ("cardiac arrest what do i do", "cpr.adult"),
    ("how deep do i press for chest compressions", "cpr.adult"),
    ("there is a defibrillator on the wall", "cpr.adult"),
    // dehydration.diarrhoea
    (
        "watery diarrhoea since this morning",
        "dehydration.diarrhoea",
    ),
    (
        "she keeps throwing up and cannot keep water down",
        "dehydration.diarrhoea",
    ),
    ("how do i mix ors at home", "dehydration.diarrhoea"),
    (
        "the baby has loose motion and a dry mouth",
        "dehydration.diarrhoea",
    ),
    ("he has passed no urine all day", "dehydration.diarrhoea"),
    // drowning.rescue
    ("we pulled him out of the water", "drowning.rescue"),
    ("the child fell in the pool", "drowning.rescue"),
    ("he was drowning in the river", "drowning.rescue"),
    ("he nearly drowned in the pond", "drowning.rescue"),
    ("she cannot swim and fell in the water", "drowning.rescue"),
    // electric.shock
    ("he touched a live wire", "electric.shock"),
    (
        "she got an electric shock from the socket",
        "electric.shock",
    ),
    ("a power line fell on the road", "electric.shock"),
    ("he has been electrocuted", "electric.shock"),
    ("the machine shocked him", "electric.shock"),
    // fracture.suspected
    ("i think his arm is broken", "fracture.suspected"),
    (
        "she twisted her ankle and cannot walk on it",
        "fracture.suspected",
    ),
    ("the bone is sticking out of his leg", "fracture.suspected"),
    ("how do i splint a broken leg", "fracture.suspected"),
    ("his leg is bent the wrong way", "fracture.suspected"),
    // head.injury
    ("he hit his head in a fall", "head.injury"),
    ("there is a big bump on the head", "head.injury"),
    ("he banged his head and feels sick", "head.injury"),
    ("concussion after a fight", "head.injury"),
    (
        "do i need to watch him for hours after a knock on the head",
        "head.injury",
    ),
    // heat.illness
    ("he collapsed working in the sun", "heat.illness"),
    ("heat stroke", "heat.illness"),
    (
        "she has been out in the heat and is confused",
        "heat.illness",
    ),
    ("he has cramps from working in the heat", "heat.illness"),
    (
        "she is dizzy and very hot after being in the sun",
        "heat.illness",
    ),
    // poisoning.swallowed
    ("the child drank kerosene", "poisoning.swallowed"),
    ("she swallowed bleach", "poisoning.swallowed"),
    ("he took an overdose", "poisoning.swallowed"),
    ("my son ate pesticide in the shed", "poisoning.swallowed"),
    (
        "should i make him sick after swallowing poison",
        "poisoning.swallowed",
    ),
    // seizure.active
    ("he is having a fit and shaking", "seizure.active"),
    ("she is jerking and foaming at the mouth", "seizure.active"),
    ("what do i do during a seizure", "seizure.active"),
    ("he has epilepsy and went stiff", "seizure.active"),
    ("the child had a convulsion with a fever", "seizure.active"),
    // snake.bite
    ("he was bitten by a snake", "snake.bite"),
    ("snake bite what do i do", "snake.bite"),
    ("should i cut and suck out the venom", "snake.bite"),
    ("a cobra bit him in the grass", "snake.bite"),
    ("bitten by something in the woodpile", "snake.bite"),
    // stroke.suspected
    ("her face has dropped on one side", "stroke.suspected"),
    ("his speech is slurred all of a sudden", "stroke.suspected"),
    (
        "he cannot lift his arm and his mouth is pulling to one side",
        "stroke.suspected",
    ),
    ("signs of a stroke", "stroke.suspected"),
    ("she has sudden weakness on one side", "stroke.suspected"),
    // unresponsive.breathing
    (
        "he will not wake up but he is breathing",
        "unresponsive.breathing",
    ),
    ("she is unconscious", "unresponsive.breathing"),
    (
        "how do i put him in the recovery position",
        "unresponsive.breathing",
    ),
    (
        "he passed out and is not answering",
        "unresponsive.breathing",
    ),
    (
        "she fainted and is still breathing",
        "unresponsive.breathing",
    ),
];

// ---------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------

/// The gate from `PLAN.md` §8, on the corpus that ships rather than a fixture.
///
/// Percentages are compared with integer arithmetic — `hits * 100 >= total * 90` — so the
/// pass mark does not move with a rounding mode. At ninety cases that is 81 for top-1 and
/// 89 for top-3.
///
/// The report is printed unconditionally so `--nocapture` shows the margins on a green run
/// too. A suite that only speaks when it fails hides the case that passed by 0.02.
#[test]
fn the_labelled_queries_meet_the_plan_gates() {
    let index = index();

    let mut top1 = 0usize;
    let mut top3 = 0usize;
    let mut report: Vec<String> = Vec::new();
    let mut narrow: Vec<(f64, String)> = Vec::new();

    for (query, expected) in CASES {
        let hits = index.search(query, 3);
        let ids = top_ids(&hits);
        match ids.iter().position(|id| id == expected) {
            Some(0) => {
                top1 += 1;
                top3 += 1;
                let first = hits[0].score;
                let margin = hits.get(1).map_or(first, |next| first - next.score);
                narrow.push((margin, format!("{margin:>6.3}  {query}")));
            }
            Some(rank) => {
                top3 += 1;
                report.push(format!(
                    "  rank {}  {query:?}\n           wanted {expected}, got {ids:?}",
                    rank + 1
                ));
            }
            None => {
                report.push(format!(
                    "  MISS    {query:?}\n           wanted {expected}, got {ids:?}"
                ));
            }
        }
    }

    let total = CASES.len();
    narrow.sort_by(|a, b| a.0.total_cmp(&b.0));

    println!("\nlabelled retrieval eval over {total} queries");
    println!(
        "  top-1  {top1}/{total}  ({:.1}%, gate 90%)",
        pct(top1, total)
    );
    println!(
        "  top-3  {top3}/{total}  ({:.1}%, gate 98%)",
        pct(top3, total)
    );
    if !report.is_empty() {
        println!("\nnot ranked first:");
        for line in &report {
            println!("{line}");
        }
    }
    println!("\nnarrowest winning margins (first place minus second):");
    for (_, line) in narrow.iter().take(8) {
        println!("  {line}");
    }
    println!();

    assert!(
        top1 * 100 >= total * 90,
        "top-1 accuracy {top1}/{total} is below the 90% gate in PLAN.md §8.\n\
         Fix the corpus or the ranker. Do not fix the queries."
    );
    assert!(
        top3 * 100 >= total * 98,
        "top-3 accuracy {top3}/{total} is below the 98% gate in PLAN.md §8.\n\
         A card missing from the top three is a card the reader will not find."
    );
}

fn pct(part: usize, whole: usize) -> f64 {
    part as f64 * 100.0 / whole as f64
}

/// A card's own search phrases must find it. This is self-consistency, not accuracy.
///
/// It is a weaker claim than the gate above and a much stronger signal when it breaks:
/// `also_called` exists for the sole purpose of being matched, so a phrase that does not
/// return its own card in the top three is either dead weight in the file or evidence that
/// another card has quietly taken that vocabulary over.
#[test]
fn every_search_phrase_finds_its_own_card() {
    let (corpus, _) = bundled::corpus();
    let index = index();

    let mut first = 0usize;
    let mut phrases = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for protocol in corpus.protocols() {
        for phrase in &protocol.also_called {
            phrases += 1;
            let hits = index.search(phrase, 3);
            let ids = top_ids(&hits);
            match ids.iter().position(|id| *id == protocol.id.as_str()) {
                Some(0) => first += 1,
                Some(_) => {}
                None => failures.push(format!(
                    "  {} lists {phrase:?} but search returns {ids:?}",
                    protocol.id
                )),
            }
        }
    }

    println!(
        "\n{first}/{phrases} search phrases rank their own card first, \
         {} outside the top three",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "a card's own search phrase does not reach its top three:\n{}",
        failures.join("\n")
    );
}

/// Every card's title finds that card first. Titles are the strongest signal in the index
/// and the phrase most likely to be read aloud off a poster or repeated by a dispatcher.
#[test]
fn every_title_ranks_its_own_card_first() {
    let (corpus, _) = bundled::corpus();
    let index = index();

    for protocol in corpus.protocols() {
        let hits = index.search(&protocol.title, 3);
        assert_eq!(
            hits.first().map(|hit| hit.protocol_id.as_str()),
            Some(protocol.id.as_str()),
            "{}'s own title ranks {:?} first",
            protocol.id,
            top_ids(&hits)
        );
    }
}

// ---------------------------------------------------------------------------
// The specific failures that shaped the ranker
// ---------------------------------------------------------------------------

/// "Heat stroke" is a heat emergency, and the corpus has to say so louder than the word
/// "stroke" does.
///
/// This is the one collision in the corpus where the red-flag layer and retrieval disagree
/// on purpose. `"stroke"` is a trigger for `rf.neuro.stroke_fast`, and "heat stroke"
/// contains it, so the rule layer shows the stroke card — documented as deliberate
/// overtriage in `redflag.rs`, and mitigated there by an `escalate_if` line on
/// `stroke.suspected` that names heat and says to start cooling. Retrieval has no such
/// constraint and must get it right: `heat.illness` was retitled "Overcome by heat" so the
/// word sits in the title field at weight 3.0 instead of only in `also_called`.
#[test]
fn heat_stroke_ranks_the_heat_card_first() {
    let index = index();
    for query in [
        "heat stroke",
        "i think he has heat stroke",
        "heat stroke or heat exhaustion",
    ] {
        let hits = index.search(query, 3);
        let ids = top_ids(&hits);
        assert_eq!(
            ids.first(),
            Some(&"heat.illness"),
            "{query:?} ranks {ids:?}; a heat emergency must not be read as a stroke"
        );
    }
}

/// The words in front of the noun do not choose the card.
///
/// Before "my" was treated as noise it appeared in exactly two cards' `also_called` — "hit
/// my head" and "pain in my chest" — where IDF read it as rare and the field weight
/// tripled it. "My child is choking" was being pulled toward a head injury by the
/// possessive. The fix is in [`prohori_core::retrieval::is_search_noise`]; this is the
/// test that says what it was for.
#[test]
fn a_possessive_or_an_article_does_not_steer_the_result() {
    let index = index();
    for (bare, spoken) in [
        ("child is choking", "my child is choking"),
        ("leg is bleeding", "his leg is bleeding"),
        ("hand is burnt", "her hand is burnt"),
        ("arm has gone weak", "that arm has gone weak"),
        ("head hit the floor", "the head hit the floor"),
    ] {
        let plain = top_ids(&index.search(bare, 3))
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let with = top_ids(&index.search(spoken, 3))
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            plain, with,
            "{bare:?} and {spoken:?} rank differently; a determiner changed the answer"
        );
    }
}

/// A misspelling reaches the same card as the correct spelling.
///
/// Retrieval shares `normalize` with the red-flag layer precisely so this holds. Someone
/// typing with one thumb, in the dark, on a cracked screen is the expected user, not the
/// edge case.
#[test]
fn a_misspelling_reaches_the_same_card() {
    let index = index();
    for (typed, correct) in [
        ("he is chocking", "he is choking"),
        ("she is having a siezure", "she is having a seizure"),
        ("he is unconcious", "he is unconscious"),
        ("the wound is bleding", "the wound is bleeding"),
        ("she is not breething", "she is not breathing"),
        ("the child drank poisin", "the child drank poison"),
        ("baby has diarhea", "baby has diarrhoea"),
    ] {
        let sloppy = top_ids(&index.search(typed, 3))
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let clean = top_ids(&index.search(correct, 3))
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            sloppy, clean,
            "{typed:?} and {correct:?} return different cards"
        );
    }
}

/// Second-language and non-clinical phrasing still lands.
///
/// The app is for anyone who reads some English, not for someone who read a first-aid
/// manual. "Loose motion", "he is finished", "took bad water" are how people actually
/// describe these things.
#[test]
fn plain_and_second_language_phrasing_still_finds_the_card() {
    let index = index();
    for (query, expected) in [
        ("baby loose motion since morning", "dehydration.diarrhoea"),
        ("boy fell in water not moving", "drowning.rescue"),
        ("man bitten snake in field", "snake.bite"),
        ("he no breathing please help", "cpr.adult"),
        ("face one side not working", "stroke.suspected"),
        ("child ate poison bottle", "poisoning.swallowed"),
        ("hot water fell on baby", "burn.thermal"),
    ] {
        let hits = index.search(query, 3);
        let ids = top_ids(&hits);
        assert!(
            ids.contains(&expected),
            "{query:?} returns {ids:?}, without {expected}"
        );
    }
}

/// A query that names two emergencies keeps both in reach.
///
/// Which one ranks first is a judgement the corpus makes and this test deliberately does
/// not assert it. What it does assert is that naming a second emergency never costs you
/// the first — the reader gets a list, and the list is where the ambiguity gets resolved
/// by a human who can see the patient.
#[test]
fn a_query_that_names_two_emergencies_offers_both() {
    let index = index();
    for (query, first, second) in [
        (
            "he hit his head and will not wake up",
            "head.injury",
            "unresponsive.breathing",
        ),
        (
            "she was stung by a bee and cannot breathe",
            "allergy.anaphylaxis",
            "breathing.distress",
        ),
        (
            "we pulled him from the water and he is not breathing",
            "cpr.adult",
            "drowning.rescue",
        ),
    ] {
        let hits = index.search(query, 3);
        let ids = top_ids(&hits);
        for wanted in [first, second] {
            assert!(
                ids.contains(&wanted),
                "{query:?} returns {ids:?}, dropping {wanted}"
            );
        }
    }
}

/// The ranker is not the safety net, and this test is here to stop anyone treating it as
/// one.
///
/// "He is not breathing" is the single most time-critical thing this app can be told, and
/// the words carry almost no ranking signal: `not` is in every card's `do_not` lines and
/// `breathe` is in most cards' steps. Retrieval handles it — `cpr.adult` is *titled* "Not
/// breathing" and the phrase index reads that pair — but it handles it by a margin, and a
/// margin is not a guarantee. Add one card about a swimming pool and the margin moves.
///
/// So the guarantee lives somewhere else. `redflag::assess` matches fixed trigger phrases
/// against a hand-written table: no scoring, no corpus size to be sensitive to, no
/// ranking to be outranked. It fires `Critical` on these queries whatever retrieval
/// happens to think, and `redflag_safety.rs` is what holds it to that.
///
/// Read together: retrieval owns the long tail of *what someone might be looking for*,
/// the rule table owns the short list of *what must never be missed*. This test asserts
/// only that the second layer is actually there and actually independent — if a future
/// change to the ranker breaks a query here, the person hurt by it is still told to call
/// an ambulance.
#[test]
fn the_rule_table_and_not_the_ranker_owns_the_critical_cases() {
    for query in [
        "he is not breathing",
        "we pulled him from the water and he is not breathing",
        "she is not breathing and has no pulse",
    ] {
        let assessment = prohori_core::redflag::assess(query);
        assert!(
            !assessment.is_empty(),
            "{query:?} fired no rule; the ranker is not allowed to be the only layer here"
        );
        assert_eq!(
            assessment.severity(),
            Some(prohori_core::severity::Severity::Critical),
            "{query:?} did not reach Critical"
        );
    }
}

// ---------------------------------------------------------------------------
// The other half of accuracy: knowing when to say nothing
// ---------------------------------------------------------------------------

/// Recall is worthless if the layer always answers. Nothing in the corpus is about a
/// phone, a bus, or opening hours, and a card returned for one of those is a card the
/// reader has been given a reason to distrust.
#[test]
fn ordinary_non_medical_english_returns_nothing() {
    let index = index();
    for query in [
        "when does the pharmacy open",
        "my phone will not charge",
        "how much does a taxi cost",
        "he is not doing very well at all",
        "can you help me please",
        "thank you so much",
        "the",
        "",
        "?????",
    ] {
        let hits = index.search(query, 3);
        assert!(
            hits.is_empty(),
            "{query:?} returned {:?}; the honest answer is nothing",
            top_ids(&hits)
        );
    }
}

/// Two builds of the same corpus rank the same way, and the same query twice gives the
/// same list.
///
/// Ordering is by a rounded integer key and then by `protocol_id`, so ties break by name
/// rather than by whatever order the index happened to be walked in. Without that, two
/// cards on identical scores could swap places between runs and the card a user was told
/// to open would depend on a hash seed.
#[test]
fn ranking_is_stable_across_builds_and_repeats() {
    let first = index();
    let second = index();
    for (query, _) in CASES {
        let a = first.search(query, 5);
        let b = first.search(query, 5);
        let c = second.search(query, 5);
        assert_eq!(top_ids(&a), top_ids(&b), "{query:?} is not repeatable");
        assert_eq!(
            top_ids(&a),
            top_ids(&c),
            "{query:?} depends on which build answered it"
        );
        for (left, right) in a.iter().zip(c.iter()) {
            assert!(
                (left.score - right.score).abs() < 1e-12,
                "{query:?} scored {} then {} for {}",
                left.score,
                right.score,
                left.protocol_id
            );
        }
    }
}

/// `limit` truncates the list; it does not reorder it.
#[test]
fn asking_for_fewer_results_returns_the_same_ones_in_the_same_order() {
    let index = index();
    for (query, _) in CASES {
        let long = top_ids(&index.search(query, 5))
            .iter()
            .take(3)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let short = top_ids(&index.search(query, 3))
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(long, short, "{query:?} reorders when the limit changes");
    }
}

/// Every hit can explain itself. `Hit::matched` is shown under the card in the UI, so an
/// empty list would be a card appearing for no stated reason.
#[test]
fn every_hit_names_the_words_that_matched() {
    let index = index();
    for (query, _) in CASES {
        for hit in index.search(query, 3) {
            assert!(
                !hit.matched.is_empty(),
                "{query:?} returned {} with nothing to show for it",
                hit.protocol_id
            );
            assert!(
                hit.score > 0.0,
                "{query:?} returned {} on a zero score",
                hit.protocol_id
            );
        }
    }
}
