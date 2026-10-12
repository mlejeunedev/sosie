//! Anonymization/pseudonymization presets.

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

/// Preset names known in v0.1. Re-exported as `config::KNOWN_PRESETS`.
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
    "siren",
    "siret",
    "vat",
];

/// Deterministic seed for a value: `HMAC-SHA256(key, preset || 0x00 || value)`.
pub struct Seed([u8; 32]);

impl Seed {
    pub fn rng(&self) -> ChaCha8Rng {
        ChaCha8Rng::from_seed(self.0)
    }
}

/// Derives a value's seed for a given preset. The same key, preset and input
/// always yield the same seed (required by `pseudonymize` mode).
pub fn seed_for(key: &[u8], preset: &str, value: &[u8]) -> Seed {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepts a key of any size");
    mac.update(preset.as_bytes());
    mac.update(&[0u8]);
    mac.update(value);
    Seed(mac.finalize().into_bytes().into())
}

/// Context passed to a preset for the column it transforms.
pub struct ColumnCtx<'k> {
    pub max_len: Option<u32>,
    /// HMAC key of the run, for presets that derive a sub-seed from part of the
    /// input (e.g. `siret` reuses the `siren` seed of its first 9 digits).
    pub key: &'k [u8],
}

/// A preset maps a non-null, non-empty value to another value,
/// deterministically for a given seed. Cross-cutting rules (`Null` → `Null`,
/// empty string → empty string, truncation to `max_len`) are applied by the
/// transformation engine, not by the preset itself.
pub trait Preset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, ctx: &ColumnCtx) -> Value<'a>;
}

fn str_owned(s: String) -> Value<'static> {
    Value::Str(Cow::Owned(s.into_bytes()))
}

/// Builds a preset from its name and parameters (from the config).
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
        "siren" => Ok(Box::new(SirenPreset)),
        "siret" => Ok(Box::new(SiretPreset)),
        "vat" => Ok(Box::new(VatPreset)),
        other => bail!("unknown preset: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Embedded data (fr_FR)
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

/// `(city, postal_code)`.
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
        // `first`/`last` only yield ~50×50 combinations: on real data volumes,
        // distinct inputs would eventually produce the same fake email (~18%
        // collisions over 1000 inputs before this fix), violating UNIQUE
        // constraints on import. A hex suffix derived from the rest of the seed
        // makes this negligible while keeping it deterministic.
        let suffix: u32 = rng.random_range(0..0x1000000);
        str_owned(format!("{first}.{last}.{suffix:06x}@example.org"))
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
        // Same letter/digit positions as the original (from the BBAN onward; the
        // two check digits are recomputed anyway).
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
        let country: String = chars[4..6].iter().collect(); // kept
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

// SIREN/SIRET/VAT are linked: a SIRET starts with its company's SIREN, and a
// French VAT number embeds it too. The fake SIREN is always derived from the
// `siren` seed of the original SIREN, so `company.siren`, `site.siret` and
// `company.vat_number` stay consistent with each other after transformation.

struct SirenPreset;
impl Preset for SirenPreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, _ctx: &ColumnCtx) -> Value<'a> {
        let original = as_str(input).unwrap_or_default();
        let original = String::from_utf8_lossy(&original);
        let fake = fake_siren(&mut seed.rng());
        str_owned(format_digits(&original, &fake))
    }
}

struct SiretPreset;
impl Preset for SiretPreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, ctx: &ColumnCtx) -> Value<'a> {
        let original = as_str(input).unwrap_or_default();
        let original = String::from_utf8_lossy(&original);
        let digits = digits_of(&original);
        let mut rng = seed.rng();
        let mut fake = if digits.len() == 14 {
            fake_siren(&mut seed_for(ctx.key, "siren", &digits[..9]).rng())
        } else {
            fake_siren(&mut rng)
        };
        // NIC (establishment number): random, from the full-SIRET seed, so that
        // two sites of the same company get distinct SIRETs.
        fake.extend((0..4).map(|_| b'0' + rng.random_range(0..10u8)));
        fake.push(luhn_check_digit(&fake));
        str_owned(format_digits(&original, &fake))
    }
}

struct VatPreset;
impl Preset for VatPreset {
    fn apply<'a>(&self, input: &Value<'a>, seed: &Seed, ctx: &ColumnCtx) -> Value<'a> {
        let original = as_str(input).unwrap_or_default();
        let original = String::from_utf8_lossy(&original);
        let compact: String = original.chars().filter(|c| !c.is_whitespace()).collect();
        let digits = digits_of(&compact);
        let is_fr =
            compact.len() == 13 && compact[..2].eq_ignore_ascii_case("FR") && digits.len() == 11;
        if is_fr {
            let siren = fake_siren(&mut seed_for(ctx.key, "siren", &digits[2..]).rng());
            let mut fake = format!("{:02}", vat_fr_key(&siren)).into_bytes();
            fake.extend_from_slice(&siren);
            return str_owned(format_digits(&original, &fake));
        }
        // Other countries: same shape, country prefix kept, digits replaced.
        let mut rng = seed.rng();
        let fake: Vec<u8> = (0..digits.len())
            .map(|_| b'0' + rng.random_range(0..10u8))
            .collect();
        str_owned(format_digits(&original, &fake))
    }
}

// ---------------------------------------------------------------------------
// IBAN: check digit recomputation (ISO 7064 mod 97-10)
// ---------------------------------------------------------------------------

/// Validates the mod-97 check digits of an IBAN (without spaces). Used by
/// `scan` for content-based detection.
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
    // Rearrange: BBAN + country + "00", letters converted to digits (A=10..Z=35).
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
// SIREN/SIRET (Luhn) and French VAT key
// ---------------------------------------------------------------------------

fn digits_of(s: &str) -> Vec<u8> {
    s.bytes().filter(u8::is_ascii_digit).collect()
}

/// Writes `digits` over the digit positions of `original`, keeping its
/// separators (`732 829 320` stays `ddd ddd ddd`). If the digit counts differ,
/// returns `digits` bare.
fn format_digits(original: &str, digits: &[u8]) -> String {
    if digits_of(original).len() != digits.len() {
        return String::from_utf8_lossy(digits).into_owned();
    }
    let mut next = digits.iter();
    original
        .chars()
        .map(|c| match c {
            '0'..='9' => *next.next().expect("same digit count") as char,
            other => other,
        })
        .collect()
}

/// Random 9-digit SIREN (first digit non-zero) with a valid Luhn key.
fn fake_siren(rng: &mut ChaCha8Rng) -> Vec<u8> {
    let mut out = vec![b'0' + rng.random_range(1..10u8)];
    out.extend((0..7).map(|_| b'0' + rng.random_range(0..10u8)));
    out.push(luhn_check_digit(&out));
    out
}

/// Digit to append to `digits` (ASCII) so that the whole passes Luhn.
fn luhn_check_digit(digits: &[u8]) -> u8 {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, d)| {
            let d = (d - b'0') as u32;
            if i % 2 == 0 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    b'0' + ((10 - sum % 10) % 10) as u8
}

/// Luhn validation of an ASCII digit string (SIREN/SIRET). Used by `scan` for
/// content-based detection.
pub fn luhn_is_valid(digits: &str) -> bool {
    let bytes = digits.as_bytes();
    !bytes.is_empty()
        && bytes.iter().all(u8::is_ascii_digit)
        && luhn_check_digit(&bytes[..bytes.len() - 1]) == bytes[bytes.len() - 1]
}

/// French VAT key: `(12 + 3 × (SIREN mod 97)) mod 97`.
fn vat_fr_key(siren: &[u8]) -> u64 {
    let n: u64 = std::str::from_utf8(siren).unwrap().parse().unwrap();
    (12 + 3 * (n % 97)) % 97
}

/// Validates a French VAT number (`FRkk` + SIREN, no spaces). Used by `scan`.
pub fn vat_fr_is_valid(vat: &str) -> bool {
    let bytes = vat.as_bytes();
    bytes.len() == 13
        && vat[..2].eq_ignore_ascii_case("FR")
        && bytes[2..].iter().all(u8::is_ascii_digit)
        && vat[2..4].parse::<u64>().ok() == Some(vat_fr_key(&bytes[4..]))
}

// ---------------------------------------------------------------------------
// Dates: civil date <-> days since an epoch, with no external dependency
// (Howard Hinnant's algorithm, public domain).
// ---------------------------------------------------------------------------

struct ParsedDate {
    year: i64,
    month: u32,
    day: u32,
    /// Time part (`HH:MM:SS[.ffffff]`), kept as is.
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
    let mp = (m as i64 + 9) % 12; // [0, 11] March=0 ... February=11
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
            &ColumnCtx {
                max_len: None,
                key: b"k",
            },
        );
        let b = EmailPreset.apply(
            &Value::Str(Cow::Borrowed(b"x")),
            &seed(),
            &ColumnCtx {
                max_len: None,
                key: b"k",
            },
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
            &ColumnCtx {
                max_len: None,
                key: b"k",
            },
        );
        let Value::Str(s) = out else {
            panic!("expected Str")
        };
        let s = String::from_utf8(s.into_owned()).unwrap();
        assert!(s.ends_with("@example.org"));
        assert!(s.contains('.'));
    }

    #[test]
    fn email_preset_does_not_collide_on_distinct_inputs() {
        // An `email` column almost always has a UNIQUE constraint in production:
        // distinct inputs must (almost) never produce the same output, or the
        // import fails.
        use std::collections::HashSet;
        let mut outputs = HashSet::new();
        for i in 0..1000 {
            let input = format!("user{i}@real-domain.example");
            let seed = seed_for(b"k", "email", input.as_bytes());
            let out = EmailPreset.apply(
                &Value::Str(Cow::Borrowed(input.as_bytes())),
                &seed,
                &ColumnCtx {
                    max_len: None,
                    key: b"k",
                },
            );
            let Value::Str(s) = out else {
                panic!("expected Str")
            };
            outputs.insert(s.into_owned());
        }
        assert_eq!(
            outputs.len(),
            1000,
            "1000 distinct inputs must give 1000 distinct outputs"
        );
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
            &ColumnCtx {
                max_len: None,
                key: b"k",
            },
        );
        let Value::Str(s) = out else {
            panic!("expected Str")
        };
        let s = String::from_utf8(s.into_owned()).unwrap();
        assert_eq!(s.len(), "1985-03-14".len());
    }

    #[test]
    fn iban_output_passes_mod97_validation() {
        let out = IbanPreset.apply(
            &Value::Str(Cow::Borrowed(b"FR7630006000011234567890189")),
            &seed_for(b"k", "iban", b"FR7630006000011234567890189"),
            &ColumnCtx {
                max_len: None,
                key: b"k",
            },
        );
        let Value::Str(s) = out else {
            panic!("expected Str")
        };
        let s = String::from_utf8(s.into_owned()).unwrap();
        assert_eq!(s.len(), "FR7630006000011234567890189".len());
        assert!(iban_is_valid(&s));
    }

    fn apply_str(preset: &dyn Preset, name: &str, input: &str) -> String {
        let out = preset.apply(
            &Value::Str(Cow::Borrowed(input.as_bytes())),
            &seed_for(b"k", name, input.as_bytes()),
            &ColumnCtx {
                max_len: None,
                key: b"k",
            },
        );
        let Value::Str(s) = out else {
            panic!("expected Str")
        };
        String::from_utf8(s.into_owned()).unwrap()
    }

    #[test]
    fn siren_siret_vat_are_valid_and_consistent() {
        let siren = apply_str(&SirenPreset, "siren", "732829320");
        let siret = apply_str(&SiretPreset, "siret", "73282932000074");
        let vat = apply_str(&VatPreset, "vat", "FR44732829320");

        assert_eq!(siren.len(), 9);
        assert!(luhn_is_valid(&siren));
        assert_ne!(siren, "732829320");
        assert!(luhn_is_valid(&siret));
        assert!(vat_fr_is_valid(&vat));

        // The same company keeps the same fake SIREN everywhere.
        assert_eq!(&siret[..9], siren);
        assert_eq!(&vat[4..], siren);
    }

    #[test]
    fn sites_of_one_company_get_distinct_sirets() {
        let a = apply_str(&SiretPreset, "siret", "73282932000074");
        let b = apply_str(&SiretPreset, "siret", "73282932000090");
        assert_eq!(a[..9], b[..9]);
        assert_ne!(a, b);
    }

    #[test]
    fn company_identifiers_keep_their_formatting() {
        let siren = apply_str(&SirenPreset, "siren", "732 829 320");
        assert!(siren.chars().nth(3) == Some(' ') && siren.chars().nth(7) == Some(' '));
        let siret = apply_str(&SiretPreset, "siret", "732 829 320 00074");
        assert_eq!(siret.len(), "732 829 320 00074".len());
        assert!(luhn_is_valid(&siret.replace(' ', "")));
        let vat = apply_str(&VatPreset, "vat", "FR 44 732829320");
        assert!(vat.starts_with("FR "));
        assert!(vat_fr_is_valid(&vat.replace(' ', "")));
        let foreign = apply_str(&VatPreset, "vat", "DE123456789");
        assert!(foreign.starts_with("DE") && foreign.len() == 11);
    }
}
