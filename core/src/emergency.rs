//! Which number to dial, and how confident we are about it.
//!
//! `PLAN.md` §0 forbids a hardcoded emergency number: the app is international, and
//! 999, 112, 911, and 000 are all correct somewhere and wrong elsewhere. §6 puts the
//! authoritative numbers in the city pack's `emergency.json`, and §7 has `ACTION_DIAL`
//! read them from there with a manual override.
//!
//! # The tension in this file, stated out loud
//!
//! There is a table of hardcoded numbers below, which looks like exactly what §0
//! prohibits. The distinction is provenance, not storage. What §0 rules out is a single
//! global `EMERGENCY_NUMBER` that the UI dials while implying local knowledge it does
//! not have. Every number here carries a [`NumberSource`], the UI renders that source,
//! and a `BuiltIn` result reads as "we have not confirmed this for where you are".
//!
//! The table earns its place because the alternative is worse. A user who has not
//! installed a pack yet, or who is travelling, still needs a button that dials
//! *something* — and the honest fallback is a labelled best guess, not a blank screen.
//!
//! # Why the fail-closed rule points the other way here
//!
//! `docs/CONVENTIONS.md` §4 says unknown resolves to the refusing answer. For a road
//! segment, refusing means "not passable". For an emergency number, refusing to show
//! anything would be the dangerous default, so the refusing answer is instead
//! [`GSM_FALLBACK`] — 112, which GSM handsets route to a local operator in most of the
//! world, frequently without a SIM and while roaming — returned with
//! [`NumberSource::GsmFallback`] so nothing pretends this is local knowledge.
//!
//! [`resolve`] therefore has no failure case and returns no `Option`. There is always a
//! number, and there is always a label saying where it came from.
//!
//! # Accuracy
//!
//! The table is best-effort and unverified. It has not been checked against a
//! telecoms authority for any country, which is precisely what [`NumberSource::BuiltIn`]
//! communicates. `PLAN.md` §10 tracks per-country verification as open work; a city
//! pack overrides this table entirely and is the only path to a `CityPack` label.

use serde::{Deserialize, Serialize};

use self::CountryEmergency as C;

/// The GSM standard emergency number.
///
/// Reachable from most GSM handsets worldwide, often without a SIM card and while
/// roaming. Not correct *everywhere*, which is why it is the last resort rather than
/// the default.
pub const GSM_FALLBACK: &str = "112";

/// Where a number came from. Rendered in the UI — `docs/CONVENTIONS.md` §9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberSource {
    /// The user typed it. They know where they are; nothing outranks this.
    UserOverride,
    /// From the installed city pack's `emergency.json`. Authoritative.
    CityPack,
    /// From the table in this module, matched on country. Unverified.
    BuiltIn,
    /// Nothing matched. [`GSM_FALLBACK`], with no claim of local accuracy.
    GsmFallback,
}

impl NumberSource {
    /// True when the number has been confirmed for the user's actual location.
    ///
    /// The UI shows a "we have not confirmed this here" line whenever this is false.
    #[must_use]
    pub fn is_confirmed_local(self) -> bool {
        matches!(self, Self::UserOverride | Self::CityPack)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserOverride => "user_override",
            Self::CityPack => "city_pack",
            Self::BuiltIn => "built_in",
            Self::GsmFallback => "gsm_fallback",
        }
    }
}

/// One country's numbers, from the built-in fallback table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountryEmergency {
    /// ISO 3166-1 alpha-2, uppercase.
    pub country: &'static str,
    /// English country name, for the "not confirmed" line in the UI.
    pub name: &'static str,
    /// The number for an ambulance. Never empty — asserted by the test suite.
    pub ambulance: &'static str,
    pub police: Option<&'static str>,
    pub fire: Option<&'static str>,
    /// True when 112 is also known to reach an operator here, so the UI can offer it
    /// as a second button rather than a replacement.
    pub gsm_112_also_works: bool,
}

/// Best-effort fallback numbers, sorted by country code.
///
/// Unverified (see module docs). Where a country runs a dedicated ambulance line
/// alongside a unified number, the dedicated line is in `ambulance` because it reaches
/// the dispatcher this app needs, and the unified number is reflected in
/// `gsm_112_also_works`.
///
/// `C` is an alias for [`CountryEmergency`], so each country stays on one readable line
/// and a diff that adds one touches exactly one line. `rustfmt` is skipped here for the
/// same reason: expanded to five lines each, the table stops being scannable, and this
/// is a table a reviewer needs to scan.
#[rustfmt::skip]
pub static COUNTRIES: &[CountryEmergency] = &[
    C { country: "AE", name: "United Arab Emirates", ambulance: "998", police: Some("999"), fire: Some("997"), gsm_112_also_works: true },
    C { country: "AR", name: "Argentina", ambulance: "107", police: Some("911"), fire: Some("100"), gsm_112_also_works: false },
    C { country: "AT", name: "Austria", ambulance: "144", police: Some("133"), fire: Some("122"), gsm_112_also_works: true },
    C { country: "AU", name: "Australia", ambulance: "000", police: Some("000"), fire: Some("000"), gsm_112_also_works: true },
    C { country: "BD", name: "Bangladesh", ambulance: "999", police: Some("999"), fire: Some("999"), gsm_112_also_works: false },
    C { country: "BE", name: "Belgium", ambulance: "112", police: Some("101"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "BR", name: "Brazil", ambulance: "192", police: Some("190"), fire: Some("193"), gsm_112_also_works: false },
    C { country: "CA", name: "Canada", ambulance: "911", police: Some("911"), fire: Some("911"), gsm_112_also_works: false },
    C { country: "CH", name: "Switzerland", ambulance: "144", police: Some("117"), fire: Some("118"), gsm_112_also_works: true },
    C { country: "CN", name: "China", ambulance: "120", police: Some("110"), fire: Some("119"), gsm_112_also_works: true },
    C { country: "CZ", name: "Czechia", ambulance: "155", police: Some("158"), fire: Some("150"), gsm_112_also_works: true },
    C { country: "DE", name: "Germany", ambulance: "112", police: Some("110"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "DK", name: "Denmark", ambulance: "112", police: Some("114"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "EG", name: "Egypt", ambulance: "123", police: Some("122"), fire: Some("180"), gsm_112_also_works: false },
    C { country: "ES", name: "Spain", ambulance: "112", police: Some("091"), fire: Some("080"), gsm_112_also_works: true },
    C { country: "FI", name: "Finland", ambulance: "112", police: Some("112"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "FR", name: "France", ambulance: "15", police: Some("17"), fire: Some("18"), gsm_112_also_works: true },
    C { country: "GB", name: "United Kingdom", ambulance: "999", police: Some("999"), fire: Some("999"), gsm_112_also_works: true },
    C { country: "GR", name: "Greece", ambulance: "166", police: Some("100"), fire: Some("199"), gsm_112_also_works: true },
    C { country: "HK", name: "Hong Kong", ambulance: "999", police: Some("999"), fire: Some("999"), gsm_112_also_works: true },
    C { country: "HU", name: "Hungary", ambulance: "104", police: Some("107"), fire: Some("105"), gsm_112_also_works: true },
    C { country: "ID", name: "Indonesia", ambulance: "119", police: Some("110"), fire: Some("113"), gsm_112_also_works: true },
    C { country: "IE", name: "Ireland", ambulance: "999", police: Some("999"), fire: Some("999"), gsm_112_also_works: true },
    C { country: "IL", name: "Israel", ambulance: "101", police: Some("100"), fire: Some("102"), gsm_112_also_works: true },
    C { country: "IN", name: "India", ambulance: "108", police: Some("100"), fire: Some("101"), gsm_112_also_works: true },
    C { country: "IT", name: "Italy", ambulance: "118", police: Some("113"), fire: Some("115"), gsm_112_also_works: true },
    C { country: "JP", name: "Japan", ambulance: "119", police: Some("110"), fire: Some("119"), gsm_112_also_works: false },
    C { country: "KE", name: "Kenya", ambulance: "999", police: Some("999"), fire: Some("999"), gsm_112_also_works: true },
    C { country: "KR", name: "South Korea", ambulance: "119", police: Some("112"), fire: Some("119"), gsm_112_also_works: true },
    C { country: "LK", name: "Sri Lanka", ambulance: "1990", police: Some("119"), fire: Some("110"), gsm_112_also_works: true },
    C { country: "MX", name: "Mexico", ambulance: "911", police: Some("911"), fire: Some("911"), gsm_112_also_works: false },
    C { country: "MY", name: "Malaysia", ambulance: "999", police: Some("999"), fire: Some("999"), gsm_112_also_works: true },
    C { country: "NG", name: "Nigeria", ambulance: "112", police: Some("112"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "NL", name: "Netherlands", ambulance: "112", police: Some("112"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "NO", name: "Norway", ambulance: "113", police: Some("112"), fire: Some("110"), gsm_112_also_works: true },
    C { country: "NP", name: "Nepal", ambulance: "102", police: Some("100"), fire: Some("101"), gsm_112_also_works: false },
    C { country: "NZ", name: "New Zealand", ambulance: "111", police: Some("111"), fire: Some("111"), gsm_112_also_works: true },
    C { country: "PH", name: "Philippines", ambulance: "911", police: Some("911"), fire: Some("911"), gsm_112_also_works: false },
    C { country: "PK", name: "Pakistan", ambulance: "1122", police: Some("15"), fire: Some("16"), gsm_112_also_works: false },
    C { country: "PL", name: "Poland", ambulance: "999", police: Some("997"), fire: Some("998"), gsm_112_also_works: true },
    C { country: "PT", name: "Portugal", ambulance: "112", police: Some("112"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "RO", name: "Romania", ambulance: "112", police: Some("112"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "RU", name: "Russia", ambulance: "103", police: Some("102"), fire: Some("101"), gsm_112_also_works: true },
    C { country: "SA", name: "Saudi Arabia", ambulance: "997", police: Some("999"), fire: Some("998"), gsm_112_also_works: true },
    C { country: "SE", name: "Sweden", ambulance: "112", police: Some("112"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "SG", name: "Singapore", ambulance: "995", police: Some("999"), fire: Some("995"), gsm_112_also_works: true },
    C { country: "TH", name: "Thailand", ambulance: "1669", police: Some("191"), fire: Some("199"), gsm_112_also_works: true },
    C { country: "TR", name: "Turkey", ambulance: "112", police: Some("112"), fire: Some("112"), gsm_112_also_works: true },
    C { country: "TW", name: "Taiwan", ambulance: "119", police: Some("110"), fire: Some("119"), gsm_112_also_works: true },
    C { country: "US", name: "United States", ambulance: "911", police: Some("911"), fire: Some("911"), gsm_112_also_works: false },
    C { country: "VN", name: "Vietnam", ambulance: "115", police: Some("113"), fire: Some("114"), gsm_112_also_works: false },
    C { country: "ZA", name: "South Africa", ambulance: "10177", police: Some("10111"), fire: Some("10177"), gsm_112_also_works: true },
];

/// The `emergency.json` block of a city pack. Authoritative when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackEmergency {
    /// ISO 3166-1 alpha-2.
    pub country: String,
    pub ambulance: String,
    #[serde(default)]
    pub police: Option<String>,
    #[serde(default)]
    pub fire: Option<String>,
}

/// A number to dial, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolved {
    pub ambulance: String,
    pub police: Option<String>,
    pub fire: Option<String>,
    /// ISO 3166-1 alpha-2 when known. `None` for the GSM fallback and for a user
    /// override with no country context.
    pub country: Option<String>,
    /// English country name when the built-in table matched, for the UI's caveat line.
    pub country_name: Option<String>,
    pub source: NumberSource,
    /// True when 112 is worth offering as a second button alongside `ambulance`.
    pub gsm_112_also_works: bool,
}

impl Resolved {
    /// The number with formatting removed, ready for a `tel:` intent.
    ///
    /// Dialers vary in what they tolerate in a `tel:` URI, and a stray space is a
    /// failed call at the worst possible moment.
    #[must_use]
    pub fn dial_string(&self) -> String {
        self.ambulance
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(c, '+' | '*' | '#'))
            .collect()
    }
}

/// Look a country up in the built-in table. Case-insensitive.
#[must_use]
pub fn for_country(code: &str) -> Option<&'static CountryEmergency> {
    let code = code.trim();
    if code.len() != 2 {
        return None;
    }
    COUNTRIES
        .iter()
        .find(|entry| entry.country.eq_ignore_ascii_case(code))
}

/// True when a string is plausibly dialable.
///
/// Deliberately permissive about separators and strict about content: a number must
/// contain at least one digit, nothing but digits and dial punctuation, and be short
/// enough to be an emergency line rather than pasted prose.
#[must_use]
pub fn is_dialable(number: &str) -> bool {
    let trimmed = number.trim();
    if trimmed.is_empty() || trimmed.len() > 20 {
        return false;
    }
    let mut has_digit = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
        } else if !matches!(ch, '+' | '*' | '#' | ' ' | '-' | '(' | ')') {
            return false;
        }
    }
    has_digit
}

/// Decide what the dial button calls.
///
/// Precedence: user override, then city pack, then the built-in table, then
/// [`GSM_FALLBACK`]. Never fails and never returns an empty number — see the module
/// docs for why refusing to produce one would be the unsafe default here.
///
/// A pack or override whose number is not [`is_dialable`] is *discarded*, not dialed.
/// Falling through to a labelled guess beats dialing something that cannot connect.
#[must_use]
pub fn resolve(
    user_override: Option<&str>,
    pack: Option<&PackEmergency>,
    country: Option<&str>,
) -> Resolved {
    if let Some(number) = user_override.filter(|n| is_dialable(n)) {
        return Resolved {
            ambulance: number.trim().to_owned(),
            police: None,
            fire: None,
            country: country.map(|c| c.trim().to_uppercase()),
            country_name: None,
            source: NumberSource::UserOverride,
            gsm_112_also_works: false,
        };
    }

    if let Some(pack) = pack.filter(|p| is_dialable(&p.ambulance)) {
        return Resolved {
            ambulance: pack.ambulance.trim().to_owned(),
            police: pack.police.clone().filter(|n| is_dialable(n)),
            fire: pack.fire.clone().filter(|n| is_dialable(n)),
            country: Some(pack.country.trim().to_uppercase()),
            country_name: for_country(&pack.country).map(|entry| entry.name.to_owned()),
            source: NumberSource::CityPack,
            gsm_112_also_works: for_country(&pack.country)
                .is_some_and(|entry| entry.gsm_112_also_works),
        };
    }

    if let Some(entry) = country.and_then(for_country) {
        return Resolved {
            ambulance: entry.ambulance.to_owned(),
            police: entry.police.map(str::to_owned),
            fire: entry.fire.map(str::to_owned),
            country: Some(entry.country.to_owned()),
            country_name: Some(entry.name.to_owned()),
            source: NumberSource::BuiltIn,
            gsm_112_also_works: entry.gsm_112_also_works,
        };
    }

    Resolved {
        ambulance: GSM_FALLBACK.to_owned(),
        police: None,
        fire: None,
        country: None,
        country_name: None,
        source: NumberSource::GsmFallback,
        gsm_112_also_works: true,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::collections::HashSet;

    // -----------------------------------------------------------------------
    // Table integrity
    // -----------------------------------------------------------------------

    /// The one thing this module must never get wrong: a country in the table with no
    /// ambulance number would render a dial button that does nothing.
    #[test]
    fn every_entry_has_a_dialable_ambulance_number() {
        for entry in COUNTRIES {
            assert!(
                is_dialable(entry.ambulance),
                "{} has ambulance {:?}, which is not dialable",
                entry.country,
                entry.ambulance
            );
        }
    }

    #[test]
    fn every_optional_number_is_dialable_when_present() {
        for entry in COUNTRIES {
            for (label, number) in [("police", entry.police), ("fire", entry.fire)] {
                if let Some(number) = number {
                    assert!(
                        is_dialable(number),
                        "{} {label} is {number:?}, which is not dialable",
                        entry.country
                    );
                }
            }
        }
    }

    #[test]
    fn country_codes_are_unique_uppercase_alpha2() {
        let mut seen = HashSet::new();
        for entry in COUNTRIES {
            assert_eq!(entry.country.len(), 2, "{:?} is not alpha-2", entry.country);
            assert!(
                entry.country.chars().all(|c| c.is_ascii_uppercase()),
                "{:?} must be uppercase",
                entry.country
            );
            assert!(
                seen.insert(entry.country),
                "duplicate country {:?}",
                entry.country
            );
            assert!(!entry.name.is_empty(), "{} has no name", entry.country);
        }
    }

    /// Sorted so a reviewer scanning for their country can find it, and so a diff that
    /// adds one shows up in one place.
    #[test]
    fn the_table_is_sorted_by_country_code() {
        let codes: Vec<&str> = COUNTRIES.iter().map(|e| e.country).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        assert_eq!(codes, sorted, "keep COUNTRIES sorted by country code");
    }

    // -----------------------------------------------------------------------
    // Lookup
    // -----------------------------------------------------------------------

    #[test]
    fn lookup_is_case_insensitive_and_trims() {
        for code in ["BD", "bd", "Bd", " bd "] {
            assert_eq!(
                for_country(code).map(|e| e.ambulance),
                Some("999"),
                "for {code:?}"
            );
        }
    }

    #[test]
    fn lookup_rejects_anything_that_is_not_alpha2() {
        for code in ["", "B", "BGD", "BANGLADESH", "  "] {
            assert!(for_country(code).is_none(), "for {code:?}");
        }
    }

    #[test]
    fn the_regional_variety_the_plan_names_is_actually_covered() {
        // PLAN.md §0 lists 999, 112, 911, 000 as all being correct somewhere.
        assert_eq!(for_country("BD").map(|e| e.ambulance), Some("999"));
        assert_eq!(for_country("DE").map(|e| e.ambulance), Some("112"));
        assert_eq!(for_country("US").map(|e| e.ambulance), Some("911"));
        assert_eq!(for_country("AU").map(|e| e.ambulance), Some("000"));
    }

    // -----------------------------------------------------------------------
    // Dialability
    // -----------------------------------------------------------------------

    #[test]
    fn dialable_accepts_real_numbers_and_rejects_junk() {
        for good in ["999", "112", "10177", "+44 999", "(02) 1234-5678", "*123#"] {
            assert!(is_dialable(good), "{good:?} should be dialable");
        }
        for bad in [
            "",
            "   ",
            "call an ambulance",
            "999 or ask a neighbour",
            "n/a",
            "-",
            "01234567890123456789012",
        ] {
            assert!(!is_dialable(bad), "{bad:?} should not be dialable");
        }
    }

    #[test]
    fn dial_string_strips_formatting_a_tel_uri_would_choke_on() {
        let resolved = resolve(Some("+44 (0) 999-111"), None, None);
        assert_eq!(resolved.dial_string(), "+440999111");
    }

    // -----------------------------------------------------------------------
    // Resolution precedence
    // -----------------------------------------------------------------------

    fn pack() -> PackEmergency {
        PackEmergency {
            country: "BD".to_owned(),
            ambulance: "10921".to_owned(),
            police: Some("999".to_owned()),
            fire: None,
        }
    }

    #[test]
    fn a_user_override_outranks_everything() {
        let resolved = resolve(Some("333"), Some(&pack()), Some("US"));
        assert_eq!(resolved.ambulance, "333");
        assert_eq!(resolved.source, NumberSource::UserOverride);
        assert!(resolved.source.is_confirmed_local());
    }

    #[test]
    fn a_pack_outranks_the_built_in_table() {
        let resolved = resolve(None, Some(&pack()), Some("US"));
        assert_eq!(resolved.ambulance, "10921");
        assert_eq!(resolved.source, NumberSource::CityPack);
        assert_eq!(resolved.country.as_deref(), Some("BD"));
        // Pack numbers are authoritative, but the country name still comes from the
        // table so the UI can name the place.
        assert_eq!(resolved.country_name.as_deref(), Some("Bangladesh"));
    }

    #[test]
    fn the_built_in_table_is_used_when_there_is_no_pack() {
        let resolved = resolve(None, None, Some("in"));
        assert_eq!(resolved.ambulance, "108");
        assert_eq!(resolved.source, NumberSource::BuiltIn);
        assert!(
            !resolved.source.is_confirmed_local(),
            "a built-in guess must not claim to be confirmed"
        );
        assert!(resolved.gsm_112_also_works);
    }

    #[test]
    fn an_unknown_country_falls_back_to_the_gsm_number_and_says_so() {
        let resolved = resolve(None, None, Some("ZZ"));
        assert_eq!(resolved.ambulance, GSM_FALLBACK);
        assert_eq!(resolved.source, NumberSource::GsmFallback);
        assert!(resolved.country.is_none());
        assert!(!resolved.source.is_confirmed_local());
    }

    #[test]
    fn no_information_at_all_still_produces_a_dialable_number() {
        let resolved = resolve(None, None, None);
        assert_eq!(resolved.ambulance, "112");
        assert!(is_dialable(&resolved.ambulance));
    }

    // -----------------------------------------------------------------------
    // Fail-closed on bad input
    // -----------------------------------------------------------------------

    /// `docs/CONVENTIONS.md` §4. A corrupt pack value is discarded rather than dialed;
    /// the button still works, one confidence level lower.
    #[test]
    fn a_pack_with_a_junk_number_is_discarded_not_dialed() {
        let bad = PackEmergency {
            country: "BD".to_owned(),
            ambulance: "ask at the desk".to_owned(),
            police: None,
            fire: None,
        };
        let resolved = resolve(None, Some(&bad), Some("BD"));
        assert_eq!(resolved.ambulance, "999", "fell through to the table");
        assert_eq!(resolved.source, NumberSource::BuiltIn);
    }

    #[test]
    fn a_junk_user_override_is_discarded_not_dialed() {
        let resolved = resolve(Some("no idea"), None, Some("GB"));
        assert_eq!(resolved.ambulance, "999");
        assert_eq!(resolved.source, NumberSource::BuiltIn);
    }

    /// A pack that lists a bad police number keeps its good ambulance number. Partial
    /// corruption must not cost the field that matters.
    #[test]
    fn one_bad_field_in_a_pack_does_not_discard_the_good_ones() {
        let partly_bad = PackEmergency {
            country: "BD".to_owned(),
            ambulance: "10921".to_owned(),
            police: Some("unknown".to_owned()),
            fire: Some("102".to_owned()),
        };
        let resolved = resolve(None, Some(&partly_bad), None);
        assert_eq!(resolved.ambulance, "10921");
        assert_eq!(resolved.source, NumberSource::CityPack);
        assert!(resolved.police.is_none(), "junk police number dropped");
        assert_eq!(resolved.fire.as_deref(), Some("102"));
    }

    #[test]
    fn a_pack_from_an_unknown_country_still_dials_its_own_number() {
        let resolved = resolve(
            None,
            Some(&PackEmergency {
                country: "ZZ".to_owned(),
                ambulance: "1234".to_owned(),
                police: None,
                fire: None,
            }),
            None,
        );
        assert_eq!(resolved.ambulance, "1234");
        assert_eq!(resolved.source, NumberSource::CityPack);
        assert!(resolved.country_name.is_none(), "no name to offer, so none");
        assert!(!resolved.gsm_112_also_works, "unknown, so do not claim it");
    }

    #[test]
    fn pack_json_round_trips() {
        let json = r#"{ "country": "BD", "ambulance": "999" }"#;
        let parsed: PackEmergency = serde_json::from_str(json).expect("parses");
        assert_eq!(parsed.ambulance, "999");
        assert!(parsed.police.is_none(), "police is optional");
    }

    #[test]
    fn source_labels_are_stable_for_traces() {
        assert_eq!(NumberSource::UserOverride.as_str(), "user_override");
        assert_eq!(NumberSource::CityPack.as_str(), "city_pack");
        assert_eq!(NumberSource::BuiltIn.as_str(), "built_in");
        assert_eq!(NumberSource::GsmFallback.as_str(), "gsm_fallback");
    }
}
