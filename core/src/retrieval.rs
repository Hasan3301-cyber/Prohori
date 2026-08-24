//! BM25 retrieval over the first-aid corpus.
//!
//! # Why there is retrieval at all when there is a rule table
//!
//! [`crate::redflag`] covers the presentations where being slow is fatal. It is a short,
//! hand-reviewed list, and it has to stay short: every phrase in it is a claim that this
//! text means an emergency, and a long list of those is a list nobody audits.
//!
//! Most of what a frightened person types is not on that list. A burn, a snake bite, a
//! child who will not stop vomiting, a bone that looks wrong — no rule fires, and before
//! this module the app answered with a shrug. That is the failure P1 closes: the corpus
//! already holds the answer, and the only thing missing was a way to find it from the
//! words someone actually used.
//!
//! # Retrieval is not triage
//!
//! `docs/CONVENTIONS.md` §10. A hit here is a *card whose words match*, nothing more. It
//! sets no severity, it bypasses no model, and it never suppresses a red flag. When both
//! layers have something to say the rule layer's card comes first and the hits come
//! after it, because a BM25 score is evidence about vocabulary and a red flag is evidence
//! about a patient.
//!
//! # Tokenizing through `normalize`
//!
//! Indexing and querying both go through [`crate::normalize::normalize`], the same
//! function the rule layer uses. That is the whole reason a second tokenizer was not
//! written: `normalize` already folds `breathin`, `breeth`, and `braething` onto
//! `breathe`, expands `cant` into `can not`, and drops determiners. Reusing it means a
//! misspelling that fires a rule also matches a document, and — more importantly — that
//! the two layers cannot drift apart. A private tokenizer here would silently diverge
//! the first time someone added a spelling fold, and the symptom would be a card that
//! stops being findable by a word that still fires a rule.
//!
//! # Warnings are indexed, and still never paraphrased
//!
//! `do_not` and `escalate_if` are excluded from [`crate::protocol::Protocol::renderable_text`]
//! because the verifier cannot see polarity: "do not put ice on it" and "put ice on it"
//! share every token. They *are* indexed here, and the two decisions do not conflict.
//! Matching a document is not reproducing its text. Someone who types "should I put ice
//! on a burn" is asking exactly the question the `do_not` line answers, and the card that
//! answers it should be findable — after which the line renders verbatim, as it always
//! did.
//!
//! # Determinism
//!
//! Scores are `f64`, and `f64::ln` is a libm call that glibc, musl, and bionic are each
//! allowed to compute to within one ulp of their own answer. Two cards separated by that
//! last bit are a tie, not a ranking, so ordering uses a quantized key ([`rank_key`]) and
//! breaks the remaining ties on protocol id. The same query returns the same order on the
//! phone as in CI, which is what makes the eval gate in `PLAN.md` §8 mean anything.

use crate::normalize::normalize;
use crate::protocol::{Corpus, Protocol};
use std::collections::{BTreeMap, BTreeSet};

/// Term-frequency saturation. The BM25 default; a card that says "chest" six times is
/// not six times more about chests than one that says it once.
const K1: f64 = 1.2;

/// Length normalization strength. The BM25 default. It matters here because the fields
/// are wildly uneven — a title is four words and a step list is two hundred.
const B: f64 = 0.75;

/// Ordering key: the score quantized to six decimal places.
///
/// See the module docs on determinism. Six places is far finer than any real score
/// difference and far coarser than the last bit of an `ln`, so it collapses
/// platform noise into ties without merging cards that genuinely rank differently.
fn rank_key(score: f64) -> i64 {
    (score * 1_000_000.0).round() as i64
}

// ---------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------

/// The parts of a card, indexed separately.
///
/// A word in the title is stronger evidence than the same word buried in step 7, and
/// BM25 alone cannot express that — it sees one flat bag of words. Weighting per field
/// (BM25F) is what makes "burn" find the burn card rather than the card that happens to
/// mention burns in a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The card heading.
    Title,
    /// Lay search vocabulary. See `data/firstaid/SCHEMA.md`.
    AlsoCalled,
    /// Who the card is for.
    AppliesTo,
    /// The instructions.
    Steps,
    /// `do_not` and `escalate_if`, together. See the module docs.
    Warnings,
}

const FIELD_COUNT: usize = 5;

impl Field {
    const ALL: [Self; FIELD_COUNT] = [
        Self::Title,
        Self::AlsoCalled,
        Self::AppliesTo,
        Self::Steps,
        Self::Warnings,
    ];

    /// How much a match in this field counts.
    ///
    /// `AlsoCalled` is weighted with the title because that is its whole purpose: it
    /// holds the words a user types for a card whose title is written for a reader who
    /// has already arrived. A `match` rather than a lookup table so the numbers sit next
    /// to the names they belong to and cannot fall out of order.
    fn weight(self) -> f64 {
        match self {
            Self::Title | Self::AlsoCalled => 3.0,
            Self::AppliesTo => 2.0,
            Self::Steps | Self::Warnings => 1.0,
        }
    }

    /// Whether this field says what the card *is*, rather than what to do about it.
    ///
    /// The three declaration fields are written to be matched: a title names the
    /// emergency, `also_called` lists the words a frightened person would use for it, and
    /// `applies_to` says who it covers. A word appearing in one of them is a claim the
    /// author made about the card's subject. `steps` and `warnings` are instructions, and a
    /// word in them may be there for any reason at all — ten cards mention choking in a
    /// `do_not` line without being about choking.
    ///
    /// [`Index::is_evidence`] leans on that difference, and it is the single most useful
    /// distinction in this module. It is not the same thing as [`Field::weight`], even
    /// though today the two happen to agree: weight decides how much a match counts once a
    /// card is a candidate, and this decides whether it is a candidate at all.
    fn is_declaration(self) -> bool {
        match self {
            Self::Title | Self::AlsoCalled | Self::AppliesTo => true,
            Self::Steps | Self::Warnings => false,
        }
    }

    /// The text of this field, ready to tokenize.
    fn text_of(self, protocol: &Protocol) -> String {
        match self {
            Self::Title => protocol.title.clone(),
            Self::AlsoCalled => protocol.also_called.join(" "),
            Self::AppliesTo => protocol.applies_to.clone(),
            Self::Steps => protocol
                .steps
                .iter()
                .map(|step| step.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            Self::Warnings => protocol
                .do_not
                .iter()
                .chain(protocol.escalate_if.iter())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// The declaration text of a card, split where a reader would pause.
///
/// One segment per `also_called` entry rather than the joined field, and `applies_to` cut at
/// sentence ends, because [`phrases`] takes adjacent word pairs and a pair straddling two
/// unrelated entries is a phrase nobody wrote. Joining "hives" and "food allergy" would
/// invent "hives food".
fn declaration_segments(protocol: &Protocol) -> Vec<&str> {
    let mut segments = vec![protocol.title.as_str()];
    segments.extend(protocol.also_called.iter().map(String::as_str));
    segments.extend(
        protocol
            .applies_to
            .split(['.', ';', ':'])
            .filter(|part| !part.trim().is_empty()),
    );
    segments
}

/// Adjacent word pairs, for the phrase index.
///
/// # Why phrases exist at all
///
/// "Cannot breathe" is among the most likely things anyone will ever type into this app,
/// and as two separate words it is nearly meaningless: `normalize` turns it into "not
/// breathe", and both of those appear in almost every card, so neither carries any IDF. Yet
/// `breathing.distress` lists "cannot breathe" in `also_called` and `cpr.adult` is *titled*
/// "Not breathing". The meaning is in the pair. Counting single words cannot see that, and
/// no amount of weighting fixes it, because the information is in the adjacency and a bag
/// of words has thrown adjacency away.
///
/// The same holds for "will not wake up", "not waking up", "under water", and "chest
/// tightness" — pairs whose halves are ordinary but whose meaning is not.
///
/// # Why both words must be searchable
///
/// A pair is indexed only when neither half is [search noise][`is_search_noise`], and the
/// eval taught that rule twice.
///
/// Allowing a pair where *either* word was real let "will not" into the index — three cards
/// have a search phrase containing it: "blood will not stop", "will not wake up", "chest
/// pain that will not go away". Three out of eighteen made it look rare and informative, so
/// "my phone will not charge" returned bleeding, unresponsive, and chest pain. A pair of
/// function words is not a phrase, it is grammar.
///
/// The same rule also let in pairs like "i put", from `burn.thermal`'s "should i put ice on
/// it". Searching "how do i put on a tourniquet" then ranked the burn card above the only
/// card in the corpus that says "tourniquet" — because a phrase scores its whole IDF while a
/// term scores a saturated fraction, so the garbage pair outweighed the exact word. A noise
/// word glued to a real one adds nothing the real word did not already say; it just gets the
/// same evidence counted a second time at a higher rate.
///
/// Note what this does *not* exclude. "Not" is deliberately absent from the noise list, so
/// "not breathe" and "not stop" are indexed and stay findable — the negation is the point.
/// What is excluded is "will not", where the negation has nothing to attach to.
fn phrases(text: &str) -> Vec<String> {
    let normalized = normalize(text);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    tokens
        .windows(2)
        .filter_map(|pair| match pair {
            [left, right] if !is_search_noise(left) && !is_search_noise(right) => {
                Some(format!("{left} {right}"))
            }
            _ => None,
        })
        .collect()
}

/// One value per [`Field`].
///
/// A newtype rather than a bare array so every access goes through [`PerField::get`],
/// which cannot panic. `docs/CONVENTIONS.md` §8 bans slice indexing in this crate, and
/// "the index is an enum discriminant so it is obviously in range" is exactly the kind of
/// obvious that stops being true when someone adds a field.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PerField<T>([T; FIELD_COUNT]);

impl<T: Copy + Default> Default for PerField<T> {
    fn default() -> Self {
        Self([T::default(); FIELD_COUNT])
    }
}

impl<T: Copy + Default> PerField<T> {
    fn get(&self, field: Field) -> T {
        self.0.get(field as usize).copied().unwrap_or_default()
    }

    fn set(&mut self, field: Field, value: T) {
        if let Some(slot) = self.0.get_mut(field as usize) {
            *slot = value;
        }
    }
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// One indexed protocol.
#[derive(Debug, Clone)]
struct Doc {
    id: String,
    field_len: PerField<u32>,
    /// Normalized `also_called` phrases used to decide whether this corpus actually
    /// covers a report. Search may suggest a card from looser body-word evidence; the
    /// model-written fallback must not be suppressed by that weaker signal.
    coverage_patterns: Vec<Vec<String>>,
}

/// One term's occurrences in one document.
#[derive(Debug, Clone)]
struct Posting {
    doc: u32,
    tf: PerField<u32>,
}

/// One card that matched, with why.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub protocol_id: String,
    /// BM25F score. Comparable within one result list and meaningless outside it — do
    /// not show it to a user, and do not compare it against a threshold of your own.
    pub score: f64,
    /// The query words that matched this card, in the order they were typed.
    ///
    /// Here for the same reason [`crate::redflag::RedFlagHit::matched`] is: a card that
    /// arrives unexplained is a card a frightened person cannot sanity-check.
    pub matched: Vec<String>,
}

/// A built BM25F index over a [`Corpus`].
///
/// Build once and keep it. Building walks every protocol and allocates; searching does
/// not, beyond the result list. The corpus is eighteen cards, so this is microseconds
/// either way — the point of holding it is that a keystroke must not re-tokenize the
/// whole corpus.
#[derive(Debug, Clone, Default)]
pub struct Index {
    /// In [`Corpus::protocols`] order, which is sorted by id.
    docs: Vec<Doc>,
    postings: BTreeMap<String, Vec<Posting>>,
    /// Word pairs from the declaration fields, to the cards that use them. See [`phrases`].
    ///
    /// No term frequencies and no field: a phrase either appears in what a card calls
    /// itself or it does not, and a card that repeated one would not thereby be more about
    /// it.
    phrase_postings: BTreeMap<String, Vec<u32>>,
    avg_field_len: PerField<f64>,
}

impl Index {
    /// Index every protocol in the corpus.
    #[must_use]
    pub fn build(corpus: &Corpus) -> Self {
        let mut docs: Vec<Doc> = Vec::new();
        // Nested maps while building so a term's postings end up sorted by document
        // without a sort, then flattened. Deterministic by construction.
        let mut building: BTreeMap<String, BTreeMap<u32, PerField<u32>>> = BTreeMap::new();
        let mut phrase_building: BTreeMap<String, BTreeMap<u32, ()>> = BTreeMap::new();

        for protocol in corpus.protocols() {
            let doc = u32::try_from(docs.len()).unwrap_or(u32::MAX);
            let mut field_len = PerField::<u32>::default();

            for field in Field::ALL {
                let normalized = normalize(&field.text_of(protocol));
                let mut length = 0u32;
                for token in normalized.split_whitespace() {
                    if is_search_noise(token) {
                        continue;
                    }
                    length = length.saturating_add(1);
                    let counts = building
                        .entry(token.to_owned())
                        .or_default()
                        .entry(doc)
                        .or_default();
                    counts.set(field, counts.get(field).saturating_add(1));
                }
                field_len.set(field, length);
            }

            for segment in declaration_segments(protocol) {
                for phrase in phrases(segment) {
                    phrase_building.entry(phrase).or_default().insert(doc, ());
                }
            }

            docs.push(Doc {
                id: protocol.id.clone(),
                field_len,
                coverage_patterns: std::iter::once(
                    protocol.title.split('—').next().unwrap_or(&protocol.title),
                )
                .chain(protocol.also_called.iter().map(String::as_str))
                .map(coverage_terms)
                .filter(|terms| !terms.is_empty())
                .collect(),
            });
        }

        let postings = building
            .into_iter()
            .map(|(term, per_doc)| {
                let list = per_doc
                    .into_iter()
                    .map(|(doc, tf)| Posting { doc, tf })
                    .collect();
                (term, list)
            })
            .collect();

        let mut avg_field_len = PerField::<f64>::default();
        if !docs.is_empty() {
            let count = docs.len() as f64;
            for field in Field::ALL {
                let total: f64 = docs
                    .iter()
                    .map(|doc| f64::from(doc.field_len.get(field)))
                    .sum();
                avg_field_len.set(field, total / count);
            }
        }

        Self {
            docs,
            postings,
            phrase_postings: phrase_building
                .into_iter()
                .map(|(phrase, per_doc)| (phrase, per_doc.into_keys().collect()))
                .collect(),
            avg_field_len,
        }
    }

    /// Number of indexed cards.
    #[must_use]
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// The best `limit` cards for `query`, best first.
    ///
    /// Empty when nothing matched well enough — see [`Index::is_evidence`]. An empty
    /// result is a real answer and the UI must render it as "we have no card for this",
    /// never as reassurance (`docs/CONVENTIONS.md` §4).
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        if limit == 0 || self.docs.is_empty() {
            return Vec::new();
        }
        let terms = query_terms(query);
        if terms.is_empty() {
            return Vec::new();
        }

        let mut scored: BTreeMap<u32, Accumulator> = BTreeMap::new();
        for (position, term) in terms.iter().enumerate() {
            // A word the corpus does not contain is not evidence against anything, so it
            // is skipped rather than scored as a miss.
            let Some(postings) = self.postings.get(term) else {
                continue;
            };
            let idf = idf(self.docs.len(), postings.len());
            let discriminating = postings.len() * 2 <= self.docs.len();

            for posting in postings {
                let Some(doc) = self.docs.get(posting.doc as usize) else {
                    continue;
                };
                let weighted = self.weighted_tf(posting, doc);
                let declared = Field::ALL
                    .into_iter()
                    .any(|field| field.is_declaration() && posting.tf.get(field) > 0);
                let accumulator = scored.entry(posting.doc).or_default();
                accumulator.score += idf * (weighted / (K1 + weighted));
                accumulator.evidence += idf;
                accumulator.discriminating += usize::from(discriminating);
                accumulator.distinctive_declaration |= declared && discriminating;
                accumulator.matched.push(position);
            }
        }

        // Phrases are scored after the words, on the whole of their IDF rather than a
        // saturated fraction of it. A pair of words appearing in a card's own title or
        // search phrases is about as strong as evidence gets in a corpus this size, and
        // unlike a term it cannot be repeated into significance — see [`phrases`].
        for phrase in phrase_terms(query) {
            let Some(docs) = self.phrase_postings.get(&phrase) else {
                continue;
            };
            let idf = idf(self.docs.len(), docs.len());
            let discriminating = docs.len() * 2 <= self.docs.len();
            for doc in docs {
                let accumulator = scored.entry(*doc).or_default();
                accumulator.score += idf;
                accumulator.evidence += idf;
                accumulator.distinctive_declaration |= discriminating;
            }
        }

        let mut hits: Vec<Hit> = scored
            .into_iter()
            .filter(|(_, accumulator)| self.is_evidence(accumulator))
            .filter_map(|(index, accumulator)| {
                let doc = self.docs.get(index as usize)?;
                Some(Hit {
                    protocol_id: doc.id.clone(),
                    score: accumulator.score,
                    matched: accumulator
                        .matched
                        .iter()
                        .filter_map(|position| terms.get(*position).cloned())
                        .collect(),
                })
            })
            .collect();

        hits.sort_by(|left, right| {
            rank_key(right.score)
                .cmp(&rank_key(left.score))
                .then_with(|| left.protocol_id.cmp(&right.protocol_id))
        });
        hits.truncate(limit);
        hits
    }

    /// Search only cards whose declared lay vocabulary is fully present in the report.
    ///
    /// This is deliberately stricter than [`Self::search`]. The ordinary search UI may
    /// offer a related card from a distinctive word in a step or from part of a declared
    /// phrase. That is useful for browsing, but it is not proof that the corpus has a
    /// protocol for the situation. Suppressing model-written fallback guidance requires
    /// one complete `also_called` phrase (order-insensitive after normalization).
    ///
    /// No score threshold is involved. Coverage is structural: every normalized word in
    /// one phrase authored for the card is present, or it is not.
    #[must_use]
    pub fn template_search(&self, query: &str, limit: usize) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let terms = coverage_terms(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let covered: BTreeSet<&str> = self
            .docs
            .iter()
            .filter(|doc| {
                doc.coverage_patterns.iter().any(|pattern| {
                    // `fit` is useful retrieval vocabulary ("having a fit"), but as a
                    // one-word coverage declaration it would classify "fit and healthy"
                    // as a seizure. The red-flag phrase `have fit` still catches the
                    // emergency form before this layer.
                    !(pattern.len() == 1 && pattern.first().is_some_and(|term| term == "fit"))
                        && pattern.iter().all(|term| terms.contains(term))
                })
            })
            .map(|doc| doc.id.as_str())
            .collect();
        if covered.is_empty() {
            return Vec::new();
        }
        self.search(query, self.docs.len())
            .into_iter()
            .filter(|hit| covered.contains(hit.protocol_id.as_str()))
            .take(limit)
            .collect()
    }

    /// Whether a match is worth showing, or is just shared English.
    ///
    /// A hit is admitted on either of two grounds:
    ///
    /// - **The card named itself.** A distinctive word, or a distinctive [word
    ///   pair][`phrases`], appears in a [declaration field][`Field::is_declaration`] —
    ///   title, `also_called`, `applies_to` — and in at most half the corpus. One is enough,
    ///   because text in those fields is there on purpose: it is the author saying "this
    ///   card is the one about that".
    /// - **Or two distinctive words in the body add up to enough.** No declaration matched,
    ///   so the case has to be made from step and warning text: at least two terms that each
    ///   point somewhere, together clearing the IDF of a term appearing in a quarter of the
    ///   corpus. That floor scales with the corpus rather than being a tuned constant.
    ///
    /// # What the eval taught, and why the rule is shaped like this
    ///
    /// The first version of this function tested rarity alone: one discriminating term plus
    /// a total-IDF floor. On eighteen cards that is wrong in both directions at once, and
    /// `tests/retrieval_quality.rs` showed it in one run.
    ///
    /// Too strict: searching "choking" returned **nothing**. Ten cards mention choking in a
    /// `do_not` line — "unless they are choking or being sick" — so its document frequency
    /// climbed past half the corpus, its IDF collapsed, and the one card actually titled
    /// "Choking" was thrown out with the rest. The most important word in the corpus was
    /// unfindable because too many cards were careful about it.
    ///
    /// Too lax: "when does the pharmacy open" returned a breathing card and a poisoning
    /// card, because "open" happens to appear in three cards' step text, and on eighteen
    /// documents that reads as a rare, informative word. One incidental body word was
    /// enough to manufacture a hit.
    ///
    /// Rarity is the wrong question at this scale. **Where** the words appear is the right
    /// one: with eighteen cards, IDF cannot tell a technical term from an ordinary word that
    /// only a few cards happened to use, but the author already answered that question by
    /// choosing which field to write it in. Everything a card claims about its own subject
    /// is in three short fields, and the phrase index reads them as phrases rather than as
    /// loose words, which is what makes "cannot breathe" findable at all.
    ///
    /// The alternative to all of this — return the best card whatever the query — is what
    /// makes a search box untrustworthy. "My phone will not charge" must not produce a CPR
    /// card, and the reason is not tidiness: a card that is obviously wrong teaches the
    /// reader that the next one might be too, and the next one might be the one that
    /// mattered.
    fn is_evidence(&self, accumulator: &Accumulator) -> bool {
        if accumulator.matched.is_empty() {
            return false;
        }
        if accumulator.distinctive_declaration {
            return true;
        }
        accumulator.discriminating >= 2
            && accumulator.evidence > idf(self.docs.len(), (self.docs.len() / 4).max(1))
    }

    /// BM25F pseudo-frequency: every field's count, weighted and length-normalized, added
    /// up before saturation rather than after.
    ///
    /// Saturating per field and then summing would let a term in three fields out-score
    /// the same term used heavily in the title, which is the opposite of the intent.
    fn weighted_tf(&self, posting: &Posting, doc: &Doc) -> f64 {
        let mut total = 0.0;
        for field in Field::ALL {
            let tf = f64::from(posting.tf.get(field));
            if tf == 0.0 {
                continue;
            }
            let average = self.avg_field_len.get(field);
            let normalizer = if average > 0.0 {
                1.0 - B + B * (f64::from(doc.field_len.get(field)) / average)
            } else {
                1.0
            };
            total += field.weight() * tf / normalizer;
        }
        total
    }
}

/// Per-document scoring state.
#[derive(Debug, Default)]
struct Accumulator {
    score: f64,
    /// Sum of the IDF of every matched term and phrase. Used only by
    /// [`Index::is_evidence`].
    evidence: f64,
    /// Matched terms appearing in at most half the corpus.
    discriminating: usize,
    /// True once the card has named itself: a distinctive word in a [declaration
    /// field][`Field::is_declaration`], or a distinctive phrase from one.
    distinctive_declaration: bool,
    /// Positions in the query's term list, in query order.
    matched: Vec<usize>,
}

/// Inverse document frequency, in the form that stays positive for every `doc_freq`.
///
/// The textbook Robertson–Spärck-Jones form goes negative once a term appears in more
/// than half the collection, which with eighteen documents happens for ordinary words
/// like "not" — and a negative IDF would let a common word *subtract* from a card's
/// score, so a card that says "not" more often would rank lower for a query containing
/// it. The `+ 1` inside the log is the standard fix.
fn idf(doc_count: usize, doc_freq: usize) -> f64 {
    let count = doc_count as f64;
    let freq = doc_freq as f64;
    ((count - freq + 0.5) / (freq + 0.5) + 1.0).ln()
}

/// Query terms in canonical form, deduplicated, in the order they were typed.
///
/// Deduplicating means "help help help" is one term. Order is kept because
/// [`Hit::matched`] is shown to a user and should read back in the order they typed.
fn query_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for token in normalize(query).split_whitespace() {
        if is_search_noise(token) {
            continue;
        }
        if !terms.iter().any(|seen| seen == token) {
            terms.push(token.to_owned());
        }
    }
    terms
}

/// Terms for deciding whether a complete declared lay phrase is present.
///
/// Unlike [`query_terms`], this keeps search-noise words. They carry structural meaning
/// inside a phrase even when they should not affect ranking: `passed out` must require
/// both words, otherwise "pain when passing urine" looks covered by the unresponsive
/// template on the strength of `pass` alone.
fn coverage_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for token in normalize(text).split_whitespace() {
        if !terms.iter().any(|seen| seen == token) {
            terms.push(token.to_owned());
        }
    }
    terms
}

/// The query's word pairs, deduplicated, in the order they were typed.
///
/// Unlike [`query_terms`] this keeps noise words inside a pair, because "not breathe" and
/// "too hot" are exactly the phrases that matter. [`phrases`] decides which pairs are worth
/// keeping at all.
fn phrase_terms(query: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for phrase in phrases(query) {
        if !seen.contains(&phrase) {
            seen.push(phrase);
        }
    }
    seen
}

/// English function words, dropped from both the index and the query.
///
/// # Why a list is needed when there is already an IDF
///
/// IDF measures how common a word is *in this corpus*, and the corpus is eighteen cards.
/// That is enough to recognise a word every card uses — "not", "them", "call" — and
/// [`Index::is_evidence`] handles those. It is not enough to recognise the other half of
/// the problem: a function word that happens to appear in exactly one card. "Sit them
/// down and watch them" is the only card that says "down", so IDF reads "down" as one of
/// the most informative words in the language, and "my phone is down" gets a head-injury
/// card. Eighteen documents cannot tell a rare function word from a technical term. A
/// list can.
///
/// The possessives are here for the same reason and are worth naming, because they were a
/// live bug rather than a tidy-up. "My" appears in exactly two cards' search phrases —
/// "hit my head" and "pain in my chest" — and those are `also_called` entries, weighted
/// 3.0. So IDF scored "my" as rare, the field weight tripled it, and every query that
/// opened "my child…" or "my leg…" drifted toward a head injury or a heart attack on the
/// strength of the possessive alone. The words a frightened person puts in front of the
/// noun should not decide which card they get.
///
/// # Why dropping words here is safe
///
/// This list is consulted by retrieval and by nothing else. [`crate::redflag`] never sees
/// it, so no phrase in the rule table can be weakened by anything written here — a word
/// dropped from the index still fires every trigger it fired before. The worst outcome of
/// a mistake in this list is a card that is harder to find by one word, not an emergency
/// that stops being recognised.
///
/// # What may be added
///
/// Closed-class words — pronouns, auxiliaries, prepositions, conjunctions — and the
/// handful of light adverbs and particles that carry no clinical meaning in any card.
/// Nothing that could be the answer to a question. The hedges at the end of the list are
/// the one group that is not closed-class, and they earn their place the same way the
/// possessives did: "think" appears in three cards, every time inside "if you think", and
/// "I think he is having a heart attack" was ranking the *stroke* card first on the
/// strength of that hedge. No card can be about thinking. Negations are deliberately
/// absent: their document frequency already flattens them, and keeping them out of this
/// list means no part of the retrieval layer can be read as touching negation.
/// `tests/corpus_integrity.rs` asserts that no card's title or search phrase is made up
/// entirely of words from this list, so the list can never render a card unfindable.
#[must_use]
pub fn is_search_noise(word: &str) -> bool {
    matches!(
        word,
        // pronouns and generic referents
        "i" | "me"
            | "you"
            | "he"
            | "him"
            | "she"
            | "we"
            | "us"
            | "they"
            | "them"
            | "it"
            | "who"
            | "whom"
            | "what"
            | "which"
            | "whose"
            | "how"
            | "why"
            | "where"
            | "someone"
            | "somebody"
            | "anyone"
            | "anybody"
            | "something"
            | "anything"
            // articles, possessive determiners, demonstratives
            | "the"
            | "a"
            | "an"
            | "my"
            | "your"
            | "his"
            | "her"
            | "our"
            | "their"
            | "its"
            | "this"
            | "that"
            | "these"
            | "those"
            // quantifiers and degree determiners
            | "all"
            | "any"
            | "some"
            | "every"
            | "each"
            | "much"
            | "many"
            | "more"
            | "most"
            | "less"
            | "other"
            | "another"
            // copula, auxiliaries, modals
            | "am"
            | "is"
            | "are"
            | "was"
            | "were"
            | "be"
            | "been"
            | "being"
            | "do"
            | "does"
            | "did"
            | "have"
            | "has"
            | "had"
            | "will"
            | "would"
            | "can"
            | "could"
            | "shall"
            | "should"
            | "may"
            | "might"
            | "must"
            // prepositions and particles
            | "of"
            | "in"
            | "on"
            | "at"
            | "to"
            | "into"
            | "onto"
            | "from"
            | "with"
            | "for"
            | "by"
            | "about"
            | "as"
            | "up"
            | "down"
            | "out"
            | "off"
            | "over"
            | "under"
            | "through"
            | "around"
            | "near"
            | "back"
            | "there"
            | "here"
            // conjunctions and connectives
            | "and"
            | "or"
            | "but"
            | "if"
            | "so"
            | "because"
            | "when"
            | "while"
            | "after"
            | "before"
            | "until"
            | "than"
            | "then"
            // light adverbs
            | "again"
            | "also"
            | "too"
            | "very"
            | "really"
            | "just"
            | "now"
            | "please"
            // hedges: a card only ever uses these as prose, never as its subject
            | "think"
            | "thinks"
            | "maybe"
            | "perhaps"
            | "probably"
            | "seems"
            // continuation: "keep doing it" is aspect, not subject matter
            | "keep"
            | "keeps"
            | "kept"
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// A protocol as JSON, so these tests exercise the same loader the app uses.
    fn card(
        id: &str,
        title: &str,
        also_called: &str,
        step: &str,
        warning: &str,
    ) -> (String, String) {
        (
            format!("{id}.json"),
            format!(
                r#"{{
                  "id": "{id}",
                  "version": "1.0.0",
                  "title": "{title}",
                  "applies_to": "Anyone.",
                  "also_called": [{also_called}],
                  "reading_grade": 4,
                  "reviewed_by": null,
                  "reviewed_at": null,
                  "citations": [{{ "source": "Test source, long enough" }}],
                  "steps": [{{ "n": 1, "kind": "assessment", "text": "{step}" }}],
                  "do_not": ["{warning}"]
                }}"#
            ),
        )
    }

    /// Eight cards, so the discriminating-term rule has a corpus to be a fraction of.
    fn index() -> Index {
        let entries = vec![
            card(
                "burn.thermal",
                "Burns",
                r#""scald", "hot oil""#,
                "Cool the burn under running water.",
                "Do not put ice on it.",
            ),
            card(
                "snake.bite",
                "Snake bite",
                r#""bitten by snake""#,
                "Keep the leg still and below the heart.",
                "Do not cut the wound.",
            ),
            card(
                "cpr.adult",
                "Not breathing",
                r#""heart attack", "collapsed""#,
                "Press hard in the middle of the chest.",
                "Do not stop to look for a pulse.",
            ),
            card(
                "choking.adult",
                "Choking",
                r#""food stuck""#,
                "Hit them between the shoulder blades.",
                "Do not reach into the mouth.",
            ),
            card(
                "fracture.suspected",
                "Broken bone",
                r#""broken arm""#,
                "Support the arm the way they are holding it.",
                "Do not try to straighten it.",
            ),
            card(
                "head.injury",
                "Head injury",
                r#""hit head""#,
                "Sit them down and watch them.",
                "Do not let them sleep alone.",
            ),
            card(
                "heat.illness",
                "Too hot",
                r#""heat stroke", "sun stroke""#,
                "Move them into the shade.",
                "Do not leave them in the sun.",
            ),
            card(
                "poisoning.swallowed",
                "Swallowed poison",
                r#""drank poison""#,
                "Find the container and keep it.",
                "Do not make them vomit.",
            ),
        ];
        let (corpus, errors) = Corpus::from_entries(entries);
        assert!(errors.is_empty(), "{errors:?}");
        Index::build(&corpus)
    }

    fn top(index: &Index, query: &str) -> Option<String> {
        index
            .search(query, 3)
            .first()
            .map(|hit| hit.protocol_id.clone())
    }

    #[test]
    fn the_index_covers_every_protocol() {
        let index = index();
        assert_eq!(index.len(), 8);
        assert!(!index.is_empty());
    }

    #[test]
    fn a_lay_phrase_finds_the_card_whose_title_never_says_it() {
        // The whole reason `also_called` exists: this card is titled "Not breathing".
        assert_eq!(top(&index(), "heart attack"), Some("cpr.adult".to_owned()));
        assert_eq!(top(&index(), "he collapsed"), Some("cpr.adult".to_owned()));
    }

    #[test]
    fn a_title_word_outranks_the_same_word_in_a_step() {
        // "arm" is in fracture's title-adjacent vocabulary and in its step; "head" is a
        // title word for head.injury. Both cards mention the other's territory.
        assert_eq!(
            top(&index(), "hit his head"),
            Some("head.injury".to_owned())
        );
        assert_eq!(
            top(&index(), "broken arm"),
            Some("fracture.suspected".to_owned())
        );
    }

    /// The module docs' claim, tested: matching a warning is not paraphrasing it.
    #[test]
    fn a_question_answered_only_by_a_warning_still_finds_the_card() {
        assert_eq!(
            top(&index(), "should i put ice on it"),
            Some("burn.thermal".to_owned())
        );
        assert_eq!(
            top(&index(), "should i make him vomit"),
            Some("poisoning.swallowed".to_owned())
        );
    }

    /// `normalize` is shared with the rule layer, so its folds apply here for free.
    #[test]
    fn a_misspelling_reaches_the_same_card() {
        let index = index();
        assert_eq!(
            top(&index, "chocking on food"),
            Some("choking.adult".to_owned())
        );
        assert_eq!(
            top(&index, "he is not breathin"),
            Some("cpr.adult".to_owned())
        );
    }

    /// The failure mode that makes a search box untrustworthy.
    #[test]
    fn junk_and_shared_english_return_nothing_rather_than_the_least_bad_card() {
        let index = index();
        for query in [
            // Deliberately not "my car broke down". `broke` folds to `break`, and
            // `fracture.suspected` is titled "Broken bone", so that query now has a real
            // one-word overlap with a card. Keeping it here would mean weakening the fold
            // to protect a test, which is backwards.
            "my phone will not charge",
            "what are your opening hours",
            "he is not doing well at all",
            "the",
            "",
            "!!!",
        ] {
            assert!(
                index.search(query, 5).is_empty(),
                "{query:?} should find no card, got {:?}",
                index.search(query, 5)
            );
        }
    }

    #[test]
    fn a_word_the_corpus_does_not_know_is_ignored_not_penalised() {
        let index = index();
        let plain = index.search("snake bite", 3);
        let padded = index.search("snake bite in the mangroves near barisal", 3);
        assert_eq!(
            plain.first().map(|hit| hit.protocol_id.clone()),
            padded.first().map(|hit| hit.protocol_id.clone()),
        );
    }

    #[test]
    fn matched_words_come_back_in_the_order_they_were_typed() {
        let index = index();
        let hit = index
            .search("cool the burn with water", 1)
            .first()
            .cloned()
            .expect("a hit");
        assert_eq!(hit.protocol_id, "burn.thermal");
        assert_eq!(hit.matched, vec!["cool", "burn", "water"]);
    }

    #[test]
    fn ranking_is_stable_across_calls_and_ties_break_on_id() {
        let index = index();
        let first = index.search("do not stop", 8);
        let second = index.search("do not stop", 8);
        assert_eq!(first, second);

        // Equal scores must not depend on hash order or insertion order.
        let mut sorted = first.clone();
        sorted.sort_by(|left, right| {
            rank_key(right.score)
                .cmp(&rank_key(left.score))
                .then_with(|| left.protocol_id.cmp(&right.protocol_id))
        });
        assert_eq!(first, sorted);
    }

    #[test]
    fn limit_is_respected_and_zero_asks_for_nothing() {
        let index = index();
        assert!(index.search("burn", 0).is_empty());
        assert!(index.search("burn", 1).len() <= 1);
    }

    #[test]
    fn an_empty_corpus_searches_to_nothing_instead_of_dividing_by_zero() {
        let index = Index::build(&Corpus::default());
        assert!(index.is_empty());
        assert!(index.search("not breathing", 5).is_empty());
    }

    /// The case that motivates [`is_search_noise`]. Only one test card says "down", so
    /// IDF alone reads it as one of the most informative words in the corpus.
    #[test]
    fn a_rare_function_word_is_not_mistaken_for_a_technical_term() {
        let index = index();
        assert!(index.search("down", 5).is_empty());
        assert!(index.search("out", 5).is_empty());
        assert!(
            !index.search("burn", 5).is_empty(),
            "a real content word must still be findable"
        );
    }

    /// Long documents must not win on length alone, which is what `B` is for.
    #[test]
    fn the_shortest_card_that_is_about_the_query_wins() {
        assert_eq!(top(&index(), "sun stroke"), Some("heat.illness".to_owned()));
    }

    // -----------------------------------------------------------------------
    // The phrase index
    // -----------------------------------------------------------------------

    /// The case that motivates [`phrases`]. "Cannot breathe" normalizes to "not breathe";
    /// every card in this corpus contains "not" in its `do_not` line and several mention
    /// breathing, so neither word can carry the query on its own. The pair can.
    #[test]
    fn a_pair_of_common_words_finds_the_card_that_is_titled_with_it() {
        let index = index();
        assert_eq!(top(&index, "cannot breathe"), Some("cpr.adult".to_owned()));
        assert_eq!(top(&index, "not breathing"), Some("cpr.adult".to_owned()));
    }

    /// A pair of function words is grammar, not a phrase, and must not be indexed —
    /// otherwise the handful of cards whose search phrases happen to contain "will not"
    /// answer "my phone will not charge".
    #[test]
    fn a_pair_of_function_words_is_not_a_phrase() {
        assert_eq!(phrases("will not wake up"), vec!["not wake".to_owned()]);
        assert!(phrases("cannot breathe").contains(&"not breathe".to_owned()));
        assert!(!phrases("cannot breathe").contains(&"can not".to_owned()));
        assert!(phrases("should i put ice on it").contains(&"put ice".to_owned()));
        assert!(
            !phrases("should i put ice on it").contains(&"i put".to_owned()),
            "a noise word glued to a real one says nothing the real word did not"
        );
        assert!(phrases("how much is it").is_empty());
    }

    /// A pair is only a phrase if someone wrote those two words in that order. Joining
    /// `also_called` entries end to end would invent phrases the author never chose —
    /// "scald" followed by "hot oil" is not a card claiming to be about "scald hot".
    #[test]
    fn a_pair_never_straddles_two_search_phrases_and_keeps_its_order() {
        let index = index();
        assert!(
            index.phrase_postings.contains_key("hot oil"),
            "a pair inside one entry is indexed"
        );
        assert!(
            !index.phrase_postings.contains_key("scald hot"),
            "a pair spanning two entries is not"
        );
        assert!(
            !index.phrase_postings.contains_key("oil hot"),
            "order is half of what a phrase means"
        );
    }

    /// Phrase matches are evidence, not a licence to answer anything.
    #[test]
    fn an_unwritten_pair_matches_nothing() {
        assert!(index().search("purple giraffe", 5).is_empty());
    }

    /// A phrase hit still has to name the words it matched, because the UI shows them and a
    /// hit that cannot explain itself is a hit the reader has no way to judge. This holds
    /// structurally: every indexed pair has two non-noise words, both of them query terms.
    #[test]
    fn a_phrase_hit_still_reports_its_matched_words() {
        let index = index();
        for hit in index.search("cannot breathe", 5) {
            assert!(!hit.matched.is_empty(), "{hit:?} matched nothing nameable");
        }
    }
}
