//! Text normalization for the red-flag layer, and for retrieval.
//!
//! The input is free English text typed by someone frightened, in a hurry, and
//! often writing in their second language — `PLAN.md` §0 makes that the design
//! target rather than an edge case. Normalization folds that variation onto a
//! canonical form so the rule table can be written once, in clean tokens, and
//! matched with plain substring search.
//!
//! # Why retrieval shares this and does not tokenize its own way
//!
//! `retrieval::Index` runs every field of every card, and every query, through
//! [`normalize`]. That is not code reuse for its own sake: it means a misspelling that
//! already reaches a red-flag rule reaches a *card* too. Two tokenizers would drift, and
//! the drift would show up as "the rule fired but search found nothing", which is the
//! most confusing possible failure to debug at 2am.
//!
//! It also puts a real obligation on [`canonical_spelling`]: there is no stemmer here, so
//! an inflection the table does not know is an inflection retrieval cannot match. A new
//! content domain therefore arrives with its folds, or its cards are unfindable by any
//! word the author happened not to type. `tests/retrieval_quality.rs` is where that shows
//! up as a failure rather than as a shrug.
//!
//! # The rule that keeps this honest
//!
//! `docs/CONVENTIONS.md` §6: **spelling variants are not synonyms.** This module
//! may fold a misspelling or an inflection onto its lemma (`breathin` → `breathe`).
//! It must never fold one word onto a *different* word (`fit` → `seizure`,
//! `heartbeat` → `pulse`). Semantic phrasing belongs in a rule's trigger list,
//! where it is visible at the point of use and individually testable.
//!
//! The reason is concrete: `fit → seizure` would make "he is fit and healthy" fire
//! the seizure card, and nobody reading the rule table would be able to see why.
//!
//! For retrieval the same rule holds, and the visible-at-the-point-of-use place is a
//! card's `also_called` list. `kid` does not fold to `child` here; a card that wants to
//! be found by "my kid is choking" says so in its own search phrases.
//!
//! # Output shape
//!
//! A space-padded, single-spaced, lowercase token string:
//! `"he is not breathing"` → `" he is not breathe "`.
//!
//! Padding both ends means [`contains_phrase`] can do word-boundary matching with
//! byte comparisons and no allocation, no regex, and no backtracking.

/// Normalize free text into a space-padded canonical token string.
///
/// Non-Latin scripts pass through as tokens rather than being dropped, so text in
/// another script degrades to "matches no English trigger" instead of panicking or
/// silently emptying.
#[must_use]
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push(' ');
    for raw in raw_tokens(input) {
        // Contractions expand first, since they can produce several tokens.
        let expanded = expand_contraction(&raw).unwrap_or(&raw);
        for word in expanded.split(' ') {
            if word.is_empty() || is_determiner(word) {
                continue;
            }
            out.push_str(canonical_spelling(word).unwrap_or(word));
            out.push(' ');
        }
    }
    out
}

/// Determiners and possessives, dropped because they carry no clinical meaning and
/// otherwise force every trigger to be written several times over
/// (`stuck in a throat` / `stuck in his throat` / `stuck in the throat`).
///
/// Negations are emphatically **not** in this list. `no`, `not`, `never`, and
/// `without` change what a sentence means about a patient, and several red-flag
/// triggers depend on them being present — see `redflag`.
fn is_determiner(word: &str) -> bool {
    matches!(
        word,
        "a" | "an"
            | "the"
            | "his"
            | "her"
            | "hers"
            | "my"
            | "mine"
            | "your"
            | "yours"
            | "their"
            | "theirs"
            | "its"
            | "our"
            | "ours"
            | "this"
            | "that"
            | "these"
            | "those"
            | "some"
    )
}

/// Lowercase, drop apostrophes so `can't` becomes `cant`, and split on everything
/// that is not alphanumeric.
fn raw_tokens(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in input.chars() {
        // Deleting rather than separating is what makes the contraction table work.
        if ch == '\'' || ch == '\u{2019}' || ch == '\u{02BC}' {
            continue;
        }
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                current.push(lower);
            }
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Word-boundary substring search over a [`normalize`]d haystack.
///
/// `haystack` must be the padded output of [`normalize`]; `phrase` is an unpadded
/// canonical phrase. Allocation-free.
#[must_use]
pub fn contains_phrase(haystack: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut from = 0usize;
    loop {
        let Some(slice) = haystack.get(from..) else {
            return false;
        };
        let Some(pos) = slice.find(phrase) else {
            return false;
        };
        let abs = from + pos;
        let before_is_space = abs.checked_sub(1).and_then(|i| bytes.get(i)) == Some(&b' ');
        let after_is_space = bytes.get(abs + phrase.len()) == Some(&b' ');
        if before_is_space && after_is_space {
            return true;
        }
        // Triggers are ASCII, so `abs` is a char boundary and `abs + 1` is too.
        from = abs + 1;
    }
}

/// Contractions, after apostrophes have been removed.
///
/// Deliberately omitted because they are ambiguous without the apostrophe:
/// `were` (we're / were), `hed` (he'd / head typo), `shed` (she'd / shed),
/// `well` (we'll / well), `ill` (I'll / ill — and `ill` is a symptom word).
fn expand_contraction(word: &str) -> Option<&'static str> {
    Some(match word {
        "cant" | "cannot" | "cnt" | "canot" => "can not",
        "isnt" => "is not",
        "arent" => "are not",
        "wasnt" => "was not",
        "werent" => "were not",
        "dont" | "dnt" => "do not",
        "doesnt" => "does not",
        "didnt" => "did not",
        "wont" => "will not",
        "wouldnt" => "would not",
        "couldnt" => "could not",
        "shouldnt" => "should not",
        "hasnt" => "has not",
        "havent" => "have not",
        "hadnt" => "had not",
        "aint" => "is not",
        "hes" => "he is",
        "shes" => "she is",
        "theyre" => "they are",
        "youre" => "you are",
        "im" => "i am",
        "ive" => "i have",
        "thats" => "that is",
        "theres" => "there is",
        "whos" => "who is",
        _ => return None,
    })
}

/// Misspellings and inflections folded onto a canonical lemma.
///
/// Every arm here must be a *spelling of the same word*. If you are tempted to add
/// `fit => "seizure"` or `heartbeat => "pulse"`, that belongs in the rule's trigger
/// list instead — see the module docs and `docs/CONVENTIONS.md` §6.
///
/// Sections up to "environment" exist for the red-flag triggers. Everything below that
/// exists for retrieval: those words appear in card titles, `also_called` phrases, and
/// `applies_to` lines, and without a fold the plural or the past tense someone types
/// misses the card entirely.
///
/// A few arms deliberately have no entry:
///
/// - `bit` does not fold to `bite`. "a bit of blood" would then match `snake.bite`.
/// - `drunk` does not fold to `drink`. It is a different word about a different problem.
/// - `sunstroke` does not fold to `stroke`. It is a heat emergency, and folding it would
///   push it into the stroke rule by spelling rather than by a trigger someone chose.
/// - `fit` is not touched at all; `spelling_folding_never_crosses_into_synonyms` depends
///   on that, and so does anyone who is fit and healthy.
fn canonical_spelling(word: &str) -> Option<&'static str> {
    Some(match word {
        // --- airway and breathing ---
        "breath" | "breathe" | "breathes" | "breathing" | "breathin" | "breathng" | "breething"
        | "breeth" | "breth" | "brethe" | "brething" | "braething" | "breathd" | "breathed" => {
            "breathe"
        }
        "gasp" | "gasps" | "gasping" | "gaspin" | "gasped" => "gasp",
        "cough" | "coughs" | "coughing" | "coughin" | "coughed" | "coff" => "cough",
        "choke" | "chokes" | "choking" | "chokin" | "choked" | "chocking" | "choaking"
        | "chokking" => "choke",
        "throat" | "throats" | "throte" | "thoat" => "throat",
        "tongue" | "tounge" | "tonge" => "tongue",
        "swallow" | "swallows" | "swallowing" | "swalow" | "swallowed" => "swallow",

        // --- circulation ---
        "pulse" | "pulses" | "puls" | "pulce" | "pulze" => "pulse",
        "heartbeat" | "heartbeats" | "heartbeet" | "hearbeat" => "heartbeat",
        "bleed" | "bleeds" | "bleeding" | "bleedin" | "bleding" | "bleading" | "bleedng"
        | "bled" | "bleedig" => "bleed",
        "blood" | "bloods" | "bloody" | "blud" | "blod" => "blood",
        "spurt" | "spurts" | "spurting" | "spurtin" | "spurted" => "spurt",
        "gush" | "gushes" | "gushing" | "gushin" | "gushed" => "gush",
        "soak" | "soaks" | "soaking" | "soaked" | "soakin" => "soak",
        "artery" | "arteries" | "artary" => "artery",

        // --- consciousness ---
        "unconscious" | "unconcious" | "unconsious" | "unconcius" | "unconscius" | "uncoscious"
        | "unconshus" => "unconscious",
        "conscious" | "concious" | "consious" | "conshus" => "conscious",
        "unresponsive" | "unresponsiv" | "unresponcive" | "unrespondsive" | "nonresponsive"
        | "unresponsove" => "unresponsive",
        "respond" | "responds" | "responding" | "respondin" | "responded" => "respond",
        "wake" | "wakes" | "waking" | "wakin" | "woke" | "waken" | "awake" => "wake",
        "collapse" | "collapses" | "collapsed" | "collapsing" | "colapsed" | "collaps"
        | "colapse" => "collapse",
        "faint" | "faints" | "fainted" | "fainting" | "faintd" => "faint",

        // --- neurological ---
        "seizure" | "seizures" | "siezure" | "siezures" | "seziure" | "seizur" | "seisure" => {
            "seizure"
        }
        "convulsion" | "convulsions" | "convulsing" | "convultion" | "convulse" | "convultions" => {
            "convulsion"
        }
        "stroke" | "strokes" | "strok" => "stroke",
        "droop" | "droops" | "drooping" | "droopin" | "drooped" | "droping" => "droop",
        "slur" | "slurs" | "slurring" | "slurred" | "slured" | "slurin" => "slur",
        "numb" | "numbness" | "numbing" | "nub" => "numb",
        "jerk" | "jerks" | "jerking" | "jerkin" | "jerked" => "jerk",
        "shake" | "shakes" | "shaking" | "shakin" | "shook" => "shake",
        "foam" | "foams" | "foaming" | "foamin" | "foamed" => "foam",

        // --- allergy ---
        "anaphylaxis" | "anaphylactic" | "anafilaxis" | "anaphalaxis" | "anaphylatic"
        | "anaphylaxsis" => "anaphylaxis",
        "swell" | "swells" | "swelling" | "swollen" | "swelled" | "sweling" | "swolen"
        | "swelln" => "swell",
        "allergy" | "allergies" | "allergic" | "alergy" | "alergic" => "allergy",
        "sting" | "stings" | "stung" | "stinging" => "sting",

        // --- environment ---
        "drown" | "drowns" | "drowning" | "drownin" | "drowned" | "drowing" | "drownd" => "drown",
        "water" | "waters" | "watter" => "water",
        "pool" | "pools" => "pool",
        "river" | "rivers" => "river",
        "swim" | "swims" | "swimming" | "swimmin" | "swam" | "swum" => "swim",
        "rescue" | "rescues" | "rescued" | "rescuing" => "rescue",

        // --- burns and heat ---
        "burn" | "burns" | "burning" | "burnt" | "burned" | "burnin" => "burn",
        "scald" | "scalds" | "scalded" | "scalding" => "scald",
        "blister" | "blisters" | "blistered" | "blistering" => "blister",
        "sunburn" | "sunburns" | "sunburnt" | "sunburned" => "sunburn",
        "cool" | "cools" | "cooled" | "cooling" | "cooler" => "cool",
        "ice" | "ices" | "iced" | "icy" => "ice",
        "heat" | "heats" | "heated" | "heating" => "heat",
        "hot" | "hotter" | "hottest" => "hot",
        "overheat" | "overheats" | "overheated" | "overheating" => "overheat",
        "sweat" | "sweats" | "sweating" | "sweated" | "sweaty" | "swet" => "sweat",
        "cramp" | "cramps" | "cramping" | "cramped" | "crampy" => "cramp",
        "exhaustion" | "exhausted" | "exausted" | "exaustion" => "exhaustion",
        "sun" | "suns" | "sunny" => "sun",

        // --- bones and limbs ---
        "break" | "breaks" | "breaking" | "broke" | "broken" | "broked" => "break",
        "bone" | "bones" | "boney" => "bone",
        "fracture" | "fractures" | "fractured" | "fracturing" | "fracure" => "fracture",
        "sprain" | "sprains" | "sprained" | "spraining" | "sprian" => "sprain",
        "twist" | "twists" | "twisted" | "twisting" => "twist",
        "dislocate" | "dislocates" | "dislocated" | "dislocation" => "dislocate",
        "splint" | "splints" | "splinted" | "splinting" => "splint",
        "limb" | "limbs" => "limb",
        "ankle" | "ankles" => "ankle",
        "wrist" | "wrists" => "wrist",
        "shoulder" | "shoulders" => "shoulder",
        "finger" | "fingers" => "finger",
        "toe" | "toes" => "toe",
        "walk" | "walks" | "walking" | "walked" | "walkin" => "walk",

        // --- poison ---
        "poison" | "poisons" | "poisoned" | "poisoning" | "poisonous" | "poisin" | "poision" => {
            "poison"
        }
        "overdose" | "overdoses" | "overdosed" | "overdosing" => "overdose",
        "pesticide" | "pesticides" | "pestiside" | "insecticide" | "insecticides" => "pesticide",
        "bleach" | "bleaches" | "bleached" => "bleach",
        "kerosene" | "kerosine" | "kerosin" => "kerosene",
        "cleaner" | "cleaners" => "cleaner",
        "fume" | "fumes" => "fume",
        "spill" | "spills" | "spilled" | "spilt" | "spilling" => "spill",
        "rinse" | "rinses" | "rinsed" | "rinsing" => "rinse",
        "container" | "containers" => "container",
        "packet" | "packets" => "packet",
        "label" | "labels" | "labelled" => "label",
        "eat" | "eats" | "eating" | "ate" | "eaten" | "eatin" => "eat",

        // --- snake bite ---
        // Respacing, not a synonym: `snakebite` is `snake bite` written closed up.
        "snakebite" | "snakebites" => "snake bite",
        "snake" | "snakes" => "snake",
        "bite" | "bites" | "bitten" | "biting" | "bited" | "bitted" => "bite",
        "venom" | "venoms" | "venomous" | "venemous" => "venom",
        "fang" | "fangs" => "fang",
        "cobra" | "cobras" => "cobra",
        "viper" | "vipers" => "viper",

        // --- electricity ---
        "electric" | "electrical" | "electricity" | "electricel" | "electic" => "electric",
        "electrocute" | "electrocutes" | "electrocuted" | "electrocution" | "electrocuting"
        | "electricuted" => "electrocute",
        "shock" | "shocks" | "shocked" | "shocking" => "shock",
        "wire" | "wires" | "wiring" => "wire",
        "socket" | "sockets" => "socket",
        "plug" | "plugs" | "plugged" => "plug",
        "current" | "currents" => "current",

        // --- head injury ---
        "head" | "heads" => "head",
        "skull" | "skulls" => "skull",
        "scalp" | "scalps" => "scalp",
        "pupil" | "pupils" => "pupil",
        "hit" | "hits" | "hitting" | "hitted" => "hit",
        "bump" | "bumps" | "bumped" | "bumping" => "bump",
        "bang" | "bangs" | "banged" | "banging" => "bang",
        "knock" | "knocks" | "knocked" | "knocking" | "knockin" => "knock",
        "black" | "blacks" | "blacked" | "blacking" => "black",
        "concussion" | "concussions" | "concussed" | "concused" | "concusion" => "concussion",
        "confuse" | "confuses" | "confused" | "confusing" | "confusion" | "confuzed" => "confuse",
        "dizzy" | "dizzier" | "dizziness" | "dizzyness" | "dizy" => "dizzy",
        "drowsy" | "drowsier" | "drowsiness" => "drowsy",

        // --- pain ---
        "pain" | "pains" | "painful" | "paining" | "painfull" => "pain",
        "hurt" | "hurts" | "hurting" | "hurted" => "hurt",
        "ache" | "aches" | "aching" | "ached" | "achy" => "ache",
        "headache" | "headaches" | "headach" => "headache",
        "crush" | "crushes" | "crushing" | "crushed" => "crush",
        "squeeze" | "squeezes" | "squeezing" | "squeezed" | "squeez" => "squeeze",
        "tight" | "tighter" | "tightly" | "tightness" | "tighten" | "tightening" | "tightend" => {
            "tight"
        }
        "nausea" | "nauseous" | "nauseated" | "nauseating" | "nausious" => "nausea",
        "jaw" | "jaws" => "jaw",
        "attack" | "attacks" | "attacked" | "attacking" => "attack",

        // --- allergy and airway, the searchable half ---
        "itch" | "itches" | "itching" | "itched" | "itchy" => "itch",
        "rash" | "rashes" => "rash",
        "hive" | "hives" => "hive",
        "wheeze" | "wheezes" | "wheezing" | "wheezed" | "wheezy" | "weeze" => "wheeze",
        "asthma" | "asthmatic" | "asthama" | "azma" => "asthma",
        "inhaler" | "inhalers" | "inhalor" => "inhaler",
        "puffer" | "puffers" => "puffer",
        "peanut" | "peanuts" => "peanut",
        "nut" | "nuts" => "nut",
        "bee" | "bees" => "bee",
        "reaction" | "reactions" => "reaction",

        // --- gut and water loss ---
        "diarrhoea" | "diarrhea" | "diarrhoeal" | "diarhoea" | "diarhea" | "diarrea"
        | "diarrhia" | "diarhhea" => "diarrhoea",
        "dehydrate" | "dehydrates" | "dehydrated" | "dehydration" | "dehidration" => "dehydrate",
        "vomit" | "vomits" | "vomited" | "vomiting" | "vomitting" | "vomitin" => "vomit",
        "throw" | "throws" | "throwing" | "threw" | "thrown" => "throw",
        "stomach" | "stomachs" | "stomache" | "stomac" => "stomach",
        "urine" | "urinate" | "urinates" | "urinating" | "urination" | "urin" => "urine",
        "stool" | "stools" => "stool",
        "drink" | "drinks" | "drinking" | "drank" | "drinkin" => "drink",
        "thirst" | "thirsts" | "thirsty" => "thirst",
        "sip" | "sips" | "sipping" | "sipped" => "sip",
        "salt" | "salts" | "salty" => "salt",
        "sugar" | "sugars" | "sugary" => "sugar",

        // --- seizure, the searchable half ---
        "twitch" | "twitches" | "twitching" | "twitched" => "twitch",
        "stiff" | "stiffer" | "stiffness" | "stiffen" | "stiffening" | "stiffened" => "stiff",
        "epilepsy" | "epileptic" | "epilepsi" => "epilepsy",

        // --- what people call the cards themselves ---
        "compression" | "compressions" => "compression",
        "resuscitation" | "resuscitate" | "resusitation" | "resucitate" => "resuscitation",
        "defibrillator" | "defibrillators" | "defib" | "defibrilator" => "defibrillator",
        "tourniquet" | "tourniquets" | "torniquet" | "turniquet" => "tourniquet",
        "stab" | "stabs" | "stabbed" | "stabbing" => "stab",
        "gunshot" | "gunshots" => "gunshot",
        "injury" | "injuries" | "injure" | "injured" | "injuring" | "injuried" => "injury",
        "wound" | "wounds" | "wounded" => "wound",
        "sign" | "signs" => "sign",
        "arrest" | "arrests" | "arrested" => "arrest",
        "speech" | "speeches" => "speech",
        "brain" | "brains" => "brain",
        "recovery" | "recover" | "recovers" | "recovered" | "recovering" => "recovery",
        "position" | "positions" | "positioned" => "position",

        // --- general verbs and body parts the triggers lean on ---
        "stop" | "stops" | "stopped" | "stoped" | "stopping" | "stopt" => "stop",
        "move" | "moves" | "moving" | "movin" | "moved" => "move",
        "talk" | "talks" | "talking" | "talkin" | "talked" => "talk",
        "speak" | "speaks" | "speaking" | "speakin" | "spoke" => "speak",
        "pass" | "passes" | "passed" | "passing" | "passd" => "pass",
        "lift" | "lifts" | "lifting" | "lifted" => "lift",
        "raise" | "raises" | "raising" | "raised" => "raise",
        "feel" | "feels" | "feeling" | "felt" | "feelin" => "feel",
        "find" | "finds" | "finding" | "found" => "find",
        "pull" | "pulls" | "pulling" | "pulled" | "pullin" => "pull",
        "fall" | "falls" | "falling" | "fell" | "fallen" => "fall",
        "stuck" | "stucked" | "stucks" => "stuck",
        "cut" | "cuts" | "cutting" => "cut",
        "arm" | "arms" => "arm",
        "leg" | "legs" => "leg",
        "eye" | "eyes" => "eye",
        "lip" | "lips" => "lip",
        "face" | "faces" => "face",
        "mouth" | "mouths" => "mouth",
        "hand" | "hands" => "hand",
        "chest" | "chests" => "chest",
        "heart" | "hearts" => "heart",
        "bad" | "badly" | "bads" => "bad",
        "lot" | "lots" => "lot",
        "weak" | "weakness" | "weakly" => "weak",
        "blue" | "bluish" => "blue",
        "pregnant" | "pregnent" | "pregnancy" => "pregnant",
        "have" | "has" | "had" | "having" | "hav" => "have",
        "take" | "takes" | "taking" | "took" | "taken" => "take",
        "go" | "goes" | "going" | "went" | "gone" | "goin" => "go",
        "get" | "gets" | "getting" | "got" | "gettin" => "get",
        "turn" | "turns" | "turning" | "turned" | "turnin" => "turn",
        "struggle" | "struggles" | "struggling" | "struggled" | "strugling" => "struggle",
        "fight" | "fights" | "fighting" | "fought" => "fight",
        "grab" | "grabs" | "grabbing" | "grabbed" | "grabbin" => "grab",
        "lose" | "loses" | "losing" | "lost" | "loosing" => "lose",
        "heavy" | "heavily" => "heavy",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_space_padded_and_single_spaced() {
        assert_eq!(normalize("Help  me   now!!"), " help me now ");
    }

    #[test]
    fn empty_input_is_still_padded() {
        assert_eq!(normalize(""), " ");
        assert_eq!(normalize("   "), " ");
        assert_eq!(normalize("!!!"), " ");
    }

    #[test]
    fn contractions_expand_so_negation_survives_punctuation_stripping() {
        // The whole point: "can't breathe" must not become the token "cant".
        assert_eq!(normalize("He can't breathe"), " he can not breathe ");
        assert_eq!(normalize("cant breath"), " can not breathe ");
        assert_eq!(normalize("She isn't responding"), " she is not respond ");
        assert_eq!(normalize("won't wake up"), " will not wake up ");
    }

    #[test]
    fn misspellings_fold_onto_the_lemma() {
        for variant in ["breathing", "breathin", "breth", "breething", "braething"] {
            assert_eq!(
                normalize(variant),
                " breathe ",
                "variant {variant} should canonicalize"
            );
        }
    }

    #[test]
    fn second_language_phrasing_reaches_the_same_tokens() {
        assert_eq!(normalize("he not breathing"), " he not breathe ");
        assert_eq!(normalize("bleeding badly"), " bleed bad ");
        assert_eq!(normalize("he not waking"), " he not wake ");
    }

    /// `docs/CONVENTIONS.md` §6. This is the test that stops the tempting shortcut.
    #[test]
    fn spelling_folding_never_crosses_into_synonyms() {
        assert!(
            !normalize("he is fit and healthy").contains("seizure"),
            "`fit` must not fold onto `seizure` — it would fire the seizure card here"
        );
        assert!(
            !normalize("no heartbeat").contains("pulse"),
            "`heartbeat` must not fold onto `pulse`; put the phrasing in the rule"
        );
        assert!(
            !normalize("convulsing").contains("seizure"),
            "`convulsion` is its own lemma, matched by its own trigger"
        );
    }

    #[test]
    fn non_latin_script_degrades_instead_of_vanishing() {
        let out = normalize("সাহায্য করুন");
        assert!(
            out.len() > 2,
            "tokens should survive, just match no trigger"
        );
        assert!(out.starts_with(' ') && out.ends_with(' '));
    }

    #[test]
    fn determiners_collapse_so_one_trigger_covers_every_possessive() {
        let expected = " stuck in throat ";
        for phrasing in [
            "stuck in his throat",
            "stuck in her throat",
            "stuck in the throat",
            "stuck in my throat",
            "stuck in their throat",
        ] {
            assert_eq!(normalize(phrasing), expected, "for {phrasing}");
        }
    }

    /// Dropping a negation would invert a red flag, so this is load-bearing.
    #[test]
    fn negations_are_never_dropped_as_function_words() {
        for word in ["no", "not", "never", "without"] {
            let out = normalize(&format!("he is {word} fine"));
            assert!(
                out.contains(&format!(" {word} ")),
                "{word} must survive normalization"
            );
        }
    }

    #[test]
    fn phrase_matching_respects_word_boundaries() {
        let hay = normalize("no pulse at all");
        assert!(contains_phrase(&hay, "no pulse"));
        assert!(
            !contains_phrase(&hay, "pul"),
            "must not match inside a word"
        );
        assert!(!contains_phrase(&hay, ""), "empty phrase never matches");
    }

    #[test]
    fn phrase_matching_finds_a_later_occurrence_after_a_partial_hit() {
        // "pulseless" comes first and must not satisfy the search for "pulse".
        let hay = normalize("pulseless then no pulse");
        assert!(contains_phrase(&hay, "pulse"));
    }
}
