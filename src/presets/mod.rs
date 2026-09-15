//! Presets d'anonymisation/pseudonymisation.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Result, bail};
use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sha2::Sha256;

use crate::dump::Value;

/// Noms de presets connus en v0.1. `config::KNOWN_PRESETS` réexporte cette liste.
pub const KNOWN_PRESETS: &[&str] = &[
    "hash",
    "email",
    "first_name",
    "last_name",
    "full_name",
    "phone",
    "date_shift",
    "iban",
    "bic",
    "address_line",
    "city",
    "postcode",
    "ip",
];

/// Graine déterministe pour une valeur donnée : `HMAC-SHA256(key, preset || 0x00 || valeur)`.
pub struct Seed([u8; 32]);

impl Seed {
    pub fn rng(&self) -> ChaCha8Rng {
        ChaCha8Rng::from_seed(self.0)
    }
}

/// Dérive la graine d'une valeur pour un preset donné. Deux appels avec la
/// même clé, le même preset et la même valeur d'entrée donnent toujours la
/// même graine (déterminisme requis par le mode `pseudonymize`).
pub fn seed_for(key: &[u8], preset: &str, value: &[u8]) -> Seed {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepte une clé de toute taille");
    mac.update(preset.as_bytes());
    mac.update(&[0u8]);
    mac.update(value);
    Seed(mac.finalize().into_bytes().into())
}

/// Contexte transmis à un preset pour la colonne qu'il transforme.
pub struct ColumnCtx {
    pub max_len: Option<u32>,
}

/// Un preset transforme une valeur non-nulle et non-vide en une autre valeur,
/// de façon déterministe pour une graine donnée. Les règles transverses
/// (`Null` → `Null`, chaîne vide → chaîne vide, troncature à `max_len`) sont
/// appliquées par le moteur de transformation, pas par le preset lui-même.
pub trait Preset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, ctx: &ColumnCtx) -> Value<'a>;
}

fn str_owned(s: String) -> Value<'static> {
    Value::Str(Cow::Owned(s.into_bytes()))
}

/// Construit un preset à partir de son nom et de ses paramètres (issus de la config).
pub fn build(name: &str, params: &BTreeMap<String, serde_yaml::Value>) -> Result<Box<dyn Preset>> {
    match name {
        "hash" => Ok(Box::new(HashPreset)),
        "email" => Ok(Box::new(EmailPreset)),
        "first_name" => Ok(Box::new(FirstNamePreset)),
        "last_name" => Ok(Box::new(LastNamePreset)),
        "full_name" => Ok(Box::new(FullNamePreset)),
        "phone" => Ok(Box::new(PhonePreset)),
        "date_shift" => {
            let days = params.get("days").and_then(|v| v.as_i64()).unwrap_or(365);
            Ok(Box::new(DateShiftPreset { days }))
        }
        "iban" => Ok(Box::new(IbanPreset)),
        "bic" => Ok(Box::new(BicPreset)),
        "address_line" => Ok(Box::new(AddressLinePreset)),
        "city" => Ok(Box::new(CityPreset)),
        "postcode" => {
            let keep_department = params
                .get("keep_department")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(Box::new(PostcodePreset { keep_department }))
        }
        "ip" => Ok(Box::new(IpPreset)),
        other => bail!("preset inconnu : {other}"),
    }
}

// ---------------------------------------------------------------------------
// Données embarquées (fr_FR)
// ---------------------------------------------------------------------------

fn lines_of(text: &'static str) -> Vec<&'static str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

fn first_names() -> &'static [&'static str] {
    static DATA: OnceLock<Vec<&'static str>> = OnceLock::new();
    DATA.get_or_init(|| lines_of(include_str!("data/fr_FR/first_names.txt")))
}

fn last_names() -> &'static [&'static str] {
    static DATA: OnceLock<Vec<&'static str>> = OnceLock::new();
    DATA.get_or_init(|| lines_of(include_str!("data/fr_FR/last_names.txt")))
}

fn streets() -> &'static [&'static str] {
    static DATA: OnceLock<Vec<&'static str>> = OnceLock::new();
    DATA.get_or_init(|| lines_of(include_str!("data/fr_FR/streets.txt")))
}

/// `(ville, code_postal)`.
fn cities() -> &'static [(&'static str, &'static str)] {
    static DATA: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    DATA.get_or_init(|| {
        lines_of(include_str!("data/fr_FR/cities.txt"))
            .into_iter()
            .filter_map(|line| line.split_once(';'))
            .collect()
    })
}

fn pick<'a, T>(rng: &mut ChaCha8Rng, items: &'a [T]) -> &'a T {
    &items[rng.random_range(0..items.len())]
}

fn as_str(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Str(s) => Some(s.to_vec()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

struct HashPreset;
impl Preset for HashPreset {
    fn apply<'a>(&self, _input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let hex: String = seed.0.iter().fold(String::new(), |mut out, b| {
            use std::fmt::Write;
            let _ = write!(out, "{b:02x}");
            out
        });
        str_owned(hex)
    }
}

struct EmailPreset;
impl Preset for EmailPreset {
    fn apply<'a>(&self, _input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let mut rng = seed.rng();
        let first = pick(&mut rng, first_names()).to_lowercase();
        let last = pick(&mut rng, last_names()).to_lowercase();
        str_owned(format!("{first}.{last}@example.org"))
    }
}

struct FirstNamePreset;
impl Preset for FirstNamePreset {
    fn apply<'a>(&self, _input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let mut rng = seed.rng();
        str_owned((*pick(&mut rng, first_names())).to_string())
    }
}

struct LastNamePreset;
impl Preset for LastNamePreset {
    fn apply<'a>(&self, _input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let mut rng = seed.rng();
        str_owned((*pick(&mut rng, last_names())).to_string())
    }
}

struct FullNamePreset;
impl Preset for FullNamePreset {
    fn apply<'a>(&self, _input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let mut rng = seed.rng();
        let first = pick(&mut rng, first_names());
        let last = pick(&mut rng, last_names());
        str_owned(format!("{first} {last}"))
    }
}

struct PhonePreset;
impl Preset for PhonePreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let original = as_str(input).unwrap_or_default();
        let original = String::from_utf8_lossy(&original);
        let mut rng = seed.rng();
        let prefix = if rng.random_bool(0.5) { '6' } else { '7' };
        let digits: Vec<u32> = (0..8).map(|_| rng.random_range(0..10)).collect();

        let formatted = if original.trim_start().starts_with('+') {
            format!(
                "+33 {prefix} {}{} {}{} {}{} {}{}",
                digits[0],
                digits[1],
                digits[2],
                digits[3],
                digits[4],
                digits[5],
                digits[6],
                digits[7]
            )
        } else {
            let rest: String = digits.iter().map(|d| d.to_string()).collect();
            format!("0{prefix}{rest}")
        };
        str_owned(formatted)
    }
}

struct DateShiftPreset {
    days: i64,
}
impl Preset for DateShiftPreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let Some(original) = as_str(input) else {
            return input.clone();
        };
        let original = String::from_utf8_lossy(&original).into_owned();
        let Some(parsed) = parse_date(&original) else {
            return input.clone();
        };
        let mut rng = seed.rng();
        let span = self.days.max(0);
        let offset = if span == 0 {
            0
        } else {
            rng.random_range(-span..=span)
        };
        let shifted_days = days_from_civil(parsed.year, parsed.month, parsed.day) + offset;
        let (y, m, d) = civil_from_days(shifted_days);
        let date_part = format!("{y:04}-{m:02}-{d:02}");
        let formatted = match parsed.time {
            Some(t) => format!("{date_part} {t}"),
            None => date_part,
        };
        str_owned(formatted)
    }
}

struct IbanPreset;
impl Preset for IbanPreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let Some(original) = as_str(input) else {
            return input.clone();
        };
        let original = String::from_utf8_lossy(&original).into_owned();
        let chars: Vec<char> = original.chars().filter(|c| !c.is_whitespace()).collect();
        if chars.len() < 8 {
            return input.clone();
        }
        let country: String = chars[0..2].iter().collect();
        let mut rng = seed.rng();
        // Mêmes positions "lettre"/"chiffre" que l'original (à partir du BBAN,
        // les 2 caractères de clé seront de toute façon recalculés).
        let mut bban: Vec<char> = chars[4..]
            .iter()
            .map(|c| {
                if c.is_ascii_alphabetic() {
                    (b'A' + rng.random_range(0..26) as u8) as char
                } else {
                    char::from_digit(rng.random_range(0..10), 10).unwrap()
                }
            })
            .collect();
        let check = iban_check_digits(&country, &bban.iter().collect::<String>());
        let mut out = String::new();
        out.push_str(&country);
        out.push_str(&check);
        bban.iter().for_each(|c| out.push(*c));
        let _ = &mut bban;
        str_owned(out)
    }
}

struct BicPreset;
impl Preset for BicPreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let Some(original) = as_str(input) else {
            return input.clone();
        };
        let original = String::from_utf8_lossy(&original).into_owned();
        let chars: Vec<char> = original.chars().collect();
        if chars.len() != 8 && chars.len() != 11 {
            return input.clone();
        }
        let mut rng = seed.rng();
        let random_letters = |rng: &mut ChaCha8Rng, n: usize| -> String {
            (0..n)
                .map(|_| (b'A' + rng.random_range(0..26) as u8) as char)
                .collect()
        };
        let bank = random_letters(&mut rng, 4);
        let country: String = chars[4..6].iter().collect(); // conservé
        let location = random_letters(&mut rng, 2);
        let branch: String = if chars.len() == 11 {
            random_letters(&mut rng, 3)
        } else {
            String::new()
        };
        str_owned(format!("{bank}{country}{location}{branch}"))
    }
}

struct AddressLinePreset;
impl Preset for AddressLinePreset {
    fn apply<'a>(&self, _input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let mut rng = seed.rng();
        let number = rng.random_range(1..200);
        let street = pick(&mut rng, streets());
        str_owned(format!("{number} {street}"))
    }
}

struct CityPreset;
impl Preset for CityPreset {
    fn apply<'a>(&self, _input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let mut rng = seed.rng();
        let (city, _) = pick(&mut rng, cities());
        str_owned((*city).to_string())
    }
}

struct PostcodePreset {
    keep_department: bool,
}
impl Preset for PostcodePreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let mut rng = seed.rng();
        if self.keep_department {
            let original = as_str(input).unwrap_or_default();
            let original = String::from_utf8_lossy(&original);
            let department: String = original.chars().take(2).collect();
            if department.len() == 2 {
                let rest: String = (0..3)
                    .map(|_| char::from_digit(rng.random_range(0..10), 10).unwrap())
                    .collect();
                return str_owned(format!("{department}{rest}"));
            }
        }
        let (_, postcode) = pick(&mut rng, cities());
        str_owned((*postcode).to_string())
    }
}

struct IpPreset;
impl Preset for IpPreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let Some(original) = as_str(input) else {
            return input.clone();
        };
        let original = String::from_utf8_lossy(&original);
        let mut rng = seed.rng();
        if original.contains(':') {
            let groups: Vec<String> = (0..4)
                .map(|_| format!("{:x}", rng.random_range(0..0xFFFFu32)))
                .collect();
            str_owned(format!("2001:db8:{}", groups.join(":")))
        } else {
            str_owned(format!("203.0.113.{}", rng.random_range(1..254)))
        }
    }
}

// ---------------------------------------------------------------------------
// IBAN : recalcul de la clé (ISO 7064 mod 97-10)
// ---------------------------------------------------------------------------

/// Vérifie la clé de contrôle mod 97 d'un IBAN (sans espaces). Utilisé par
/// `scan` pour la détection par contenu.
pub fn iban_is_valid(iban: &str) -> bool {
    if iban.len() < 8 {
        return false;
    }
    let country = &iban[0..2];
    let check = &iban[2..4];
    let bban = &iban[4..];
    iban_check_digits(country, bban) == check
}

fn iban_check_digits(country: &str, bban: &str) -> String {
    // Réarrangement : BBAN + pays + "00", lettres converties en chiffres (A=10..Z=35).
    let rearranged = format!("{bban}{country}00");
    let mut numeric = String::with_capacity(rearranged.len() * 2);
    for c in rearranged.chars() {
        if c.is_ascii_digit() {
            numeric.push(c);
        } else {
            numeric.push_str(&(c.to_ascii_uppercase() as u32 - 'A' as u32 + 10).to_string());
        }
    }
    let mut remainder: u64 = 0;
    for c in numeric.chars() {
        let digit = c.to_digit(10).unwrap() as u64;
        remainder = (remainder * 10 + digit) % 97;
    }
    let check = 98 - remainder;
    format!("{check:02}")
}

// ---------------------------------------------------------------------------
// Dates : conversion jour civil <-> nombre de jours depuis une époque, sans
// dépendance externe (algorithme d'Howard Hinnant, domaine public).
// ---------------------------------------------------------------------------

struct ParsedDate {
    year: i64,
    month: u32,
    day: u32,
    /// Partie horaire (`HH:MM:SS[.ffffff]`), conservée telle quelle.
    time: Option<String>,
}

fn parse_date(s: &str) -> Option<ParsedDate> {
    let (date_part, time_part) = match s.split_once(' ') {
        Some((d, t)) => (d, Some(t.to_string())),
        None => (s, None),
    };
    let mut it = date_part.splitn(3, '-');
    let year: i64 = it.next()?.parse().ok()?;
    let month: u32 = it.next()?.parse().ok()?;
    let day: u32 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(ParsedDate {
        year,
        month,
        day,
        time: time_part,
    })
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0, 11] mars=0 ... fevrier=11
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> Seed {
        seed_for(b"test-key", "test", b"value")
    }

    #[test]
    fn same_input_gives_same_output() {
        let a = EmailPreset.apply(
            &Value::Str(Cow::Borrowed(b"x")),
            &seed(),
            &ColumnCtx { max_len: None },
        );
        let b = EmailPreset.apply(
            &Value::Str(Cow::Borrowed(b"x")),
            &seed(),
            &ColumnCtx { max_len: None },
        );
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn different_preset_name_gives_different_seed() {
        let s1 = seed_for(b"k", "email", b"a@b.fr");
        let s2 = seed_for(b"k", "phone", b"a@b.fr");
        assert_ne!(s1.0, s2.0);
    }

    #[test]
    fn email_preset_produces_expected_shape() {
        let out = EmailPreset.apply(
            &Value::Str(Cow::Borrowed(b"whatever")),
            &seed(),
            &ColumnCtx { max_len: None },
        );
        let Value::Str(s) = out else {
            panic!("attendu Str")
        };
        let s = String::from_utf8(s.into_owned()).unwrap();
        assert!(s.ends_with("@example.org"));
        assert!(s.contains('.'));
    }

    #[test]
    fn date_shift_round_trip_of_civil_calendar() {
        for (y, m, d) in [(2024, 2, 29), (2000, 1, 1), (1985, 3, 14), (2023, 12, 31)] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d));
        }
    }

    #[test]
    fn date_shift_preserves_format() {
        let preset = DateShiftPreset { days: 30 };
        let out = preset.apply(
            &Value::Str(Cow::Borrowed(b"1985-03-14")),
            &seed_for(b"k", "date_shift", b"1985-03-14"),
            &ColumnCtx { max_len: None },
        );
        let Value::Str(s) = out else {
            panic!("attendu Str")
        };
        let s = String::from_utf8(s.into_owned()).unwrap();
        assert_eq!(s.len(), "1985-03-14".len());
    }

    #[test]
    fn iban_output_passes_mod97_validation() {
        let out = IbanPreset.apply(
            &Value::Str(Cow::Borrowed(b"FR7630006000011234567890189")),
            &seed_for(b"k", "iban", b"FR7630006000011234567890189"),
            &ColumnCtx { max_len: None },
        );
        let Value::Str(s) = out else {
            panic!("attendu Str")
        };
        let s = String::from_utf8(s.into_owned()).unwrap();
        assert_eq!(s.len(), "FR7630006000011234567890189".len());
        assert!(iban_is_valid(&s));
    }
}
