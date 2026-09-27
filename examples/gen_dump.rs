//! Generates a large, realistic synthetic `mysqldump` dump (classicmodels
//! schema + `user` table) for testing sosie at scale.
//!
//! ```text
//! cargo run --release --example gen_dump -- --size 1G --out fixtures/big/big.sql
//! sosie transform --from fixtures/big/big.sql --config sosie.yaml --out /dev/null
//! ```
//!
//! Output is deterministic for a given `--seed`. Data is varied (accents,
//! escaped quotes, `NULL`, JSON, newlines), foreign keys are consistent and
//! `user.email` is unique.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

#[derive(Parser, Debug)]
#[command(about = "Génère un gros dump mysqldump synthétique pour tester sosie.")]
struct Args {
    /// Target file size: `500M`, `1G`, `2G`… (approximate, ±2%).
    #[arg(long, default_value = "1G")]
    size: String,
    /// Output file.
    #[arg(long, default_value = "fixtures/big/big.sql")]
    out: PathBuf,
    /// Generator seed (same seed = same dump).
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim().to_ascii_uppercase();
    let (num, mult) = match s.chars().last() {
        Some('K') => (&s[..s.len() - 1], 1u64 << 10),
        Some('M') => (&s[..s.len() - 1], 1u64 << 20),
        Some('G') => (&s[..s.len() - 1], 1u64 << 30),
        Some(c) if c.is_ascii_digit() => (s.as_str(), 1),
        _ => bail!("taille invalide : {s} (attendu ex. 500M, 1G)"),
    };
    let n: f64 = num
        .parse()
        .with_context(|| format!("taille invalide : {s}"))?;
    Ok((n * mult as f64) as u64)
}

/// Maximum size of an extended `INSERT`, as with `mysqldump --net-buffer-length`.
const INSERT_MAX_BYTES: usize = 1 << 20;

// --- Vocabulary ----------------------------------------------------------

const FIRST_NAMES: &[&str] = &[
    "Jean",
    "Marie",
    "Pierre",
    "Sophie",
    "Lucas",
    "Camille",
    "Nicolas",
    "Julie",
    "Thomas",
    "Émilie",
    "Antoine",
    "Chloé",
    "François",
    "Léa",
    "Mathieu",
    "Inès",
    "Hugo",
    "Anaïs",
    "Louis",
    "Zoé",
    "Gaël",
    "Noémie",
    "Yann",
    "Océane",
    "Rémi",
    "Aurélie",
    "Loïc",
    "Maëlle",
    "Jérôme",
    "Élodie",
    "Karim",
    "Fatima",
    "Mohamed",
    "Aïcha",
    "Ahmed",
    "Nadia",
    "José",
    "Mia",
    "Enzo",
    "Clémence",
];
const LAST_NAMES: &[&str] = &[
    "Martin",
    "Bernard",
    "Dubois",
    "Thomas",
    "Robert",
    "Richard",
    "Petit",
    "Durand",
    "Leroy",
    "Moreau",
    "Simon",
    "Laurent",
    "Lefebvre",
    "Michel",
    "Garcia",
    "David",
    "Bertrand",
    "Roux",
    "Vincent",
    "Fournier",
    "Morel",
    "Girard",
    "André",
    "Lefèvre",
    "Mercier",
    "Dupont",
    "Lambert",
    "Bonnet",
    "François",
    "Martinez",
    "O'Connor",
    "D'Angelo",
    "Le Gall",
    "N'Guyen",
    "Da Silva",
    "Müller",
    "Øster",
    "Benali",
    "Haddad",
    "Nkemelu",
];
const STREETS: &[&str] = &[
    "rue de la République",
    "avenue des Champs-Élysées",
    "boulevard Saint-Germain",
    "rue du Faubourg Saint-Honoré",
    "place de l'Étoile",
    "rue de l'Église",
    "chemin des Dames",
    "allée des Peupliers",
    "impasse du Moulin",
    "quai de la Tournelle",
    "rue Victor Hugo",
    "avenue Jean Jaurès",
    "rue du 8 Mai 1945",
    "cours Lafayette",
    "rue d'Alsace-Lorraine",
];
const CITIES: &[(&str, &str, &str)] = &[
    ("Paris", "75001", "Île-de-France"),
    ("Lyon", "69001", "Auvergne-Rhône-Alpes"),
    ("Marseille", "13001", "Provence-Alpes-Côte d'Azur"),
    ("Toulouse", "31000", "Occitanie"),
    ("Nice", "06000", "Provence-Alpes-Côte d'Azur"),
    ("Nantes", "44000", "Pays de la Loire"),
    ("Strasbourg", "67000", "Grand Est"),
    ("Montpellier", "34000", "Occitanie"),
    ("Bordeaux", "33000", "Nouvelle-Aquitaine"),
    ("Lille", "59000", "Hauts-de-France"),
    ("Rennes", "35000", "Bretagne"),
    ("Reims", "51100", "Grand Est"),
    ("Saint-Étienne", "42000", "Auvergne-Rhône-Alpes"),
    ("Le Havre", "76600", "Normandie"),
    ("Grenoble", "38000", "Auvergne-Rhône-Alpes"),
    ("Dijon", "21000", "Bourgogne-Franche-Comté"),
    ("Angers", "49000", "Pays de la Loire"),
    ("Nîmes", "30000", "Occitanie"),
    ("Aix-en-Provence", "13100", "Provence-Alpes-Côte d'Azur"),
    ("Brest", "29200", "Bretagne"),
];
const COUNTRIES: &[&str] = &["France", "France", "France", "Belgique", "Suisse", "Canada"];
const COMPANY_A: &[&str] = &[
    "Boutique",
    "Atelier",
    "Maison",
    "Galerie",
    "Comptoir",
    "Société",
    "Garage",
    "Studio",
    "Librairie",
    "Épicerie",
    "Cabinet",
    "Agence",
];
const COMPANY_B: &[&str] = &[
    "du Centre",
    "de la Gare",
    "des Arts",
    "Moderne",
    "& Fils",
    "Dupont",
    "de l'Ouest",
    "Royale",
    "du Marché",
    "Saint-Michel",
    "Lumière",
    "Horizon",
];
const DOMAINS: &[&str] = &[
    "gmail.com",
    "orange.fr",
    "free.fr",
    "hotmail.fr",
    "yahoo.fr",
    "outlook.com",
    "laposte.net",
    "sfr.fr",
    "protonmail.com",
    "wanadoo.fr",
];
const JOB_TITLES: &[&str] = &[
    "Sales Rep",
    "Sales Rep",
    "Sales Rep",
    "Sales Manager (EMEA)",
    "VP Sales",
    "VP Marketing",
    "Sales Manager (NA)",
    "President",
];
const ORDER_STATUS: &[&str] = &[
    "Shipped",
    "Shipped",
    "Shipped",
    "Shipped",
    "Resolved",
    "Cancelled",
    "On Hold",
    "Disputed",
    "In Process",
];
const COMMENTS: &[Option<&str>] = &[
    None,
    None,
    None,
    Some("Check on availability."),
    Some("Difficult to negotiate with customer. We need more marketing materials"),
    Some("Customer requested that FedEx Ground is used for this shipping"),
    Some("Livraison à l'arrière du bâtiment.\nSonner à l'interphone « Réception »."),
    Some("Client absent le 15/08 ; rappeler la semaine suivante."),
    Some("Adresse de facturation ≠ adresse de livraison"),
    Some("Colis fragile : « HAUT » / « BAS » — merci de respecter le sens"),
    Some("Remise 10% négociée par tél. (voir mail du 03/02)"),
];
const PRODUCT_LINES: &[(&str, &str)] = &[
    (
        "Classic Cars",
        "Attention car enthusiasts: Make your wildest car-owning dreams come true.",
    ),
    (
        "Motorcycles",
        "Our motorcycles are state of the art replicas of classic as well as contemporary motorcycle legends.",
    ),
    (
        "Planes",
        "Unique, diecast airplane and helicopter replicas suitable for collections.",
    ),
    (
        "Ships",
        "The perfect holiday or anniversary gift for executives, clients, friends, and family.",
    ),
    (
        "Trains",
        "Model trains are a rewarding hobby for enthusiasts of all ages.",
    ),
    (
        "Trucks and Buses",
        "The Truck and Bus models are realistic replicas of buses and specialized trucks produced from the early 1920s to present.",
    ),
    (
        "Vintage Cars",
        "Our Vintage Car models realistically portray automobiles produced from the early 1900s through the 1940s.",
    ),
];
const PRODUCT_ADJ: &[&str] = &[
    "1952", "1969", "1936", "1998", "2001", "1957", "1972", "1912", "1980s", "1962",
];
const PRODUCT_NOUN: &[&str] = &[
    "Alpine Renault 1300",
    "Harley Davidson Ultimate Chopper",
    "Ford Falcon",
    "Corvette",
    "Chevy Pickup",
    "Porsche 356-A Roadster",
    "Mercedes-Benz 500K",
    "Ducati Monster",
    "Boeing X-32A JSF",
    "Titanic",
    "Diesel Locomotive",
    "Volkswagen Beetle",
    "Citroën DS",
    "Peugeot 404",
    "Renault 4L",
    "Dodge Ram Pickup",
    "Corsair F4U",
    "Mayflower",
];
const VENDORS: &[&str] = &[
    "Min Lin Diecast",
    "Classic Metal Creations",
    "Highway 66 Mini Classics",
    "Red Start Diecast",
    "Motor City Art Classics",
    "Unimax Art Galleries",
    "Exoto Designs",
    "Gearbox Collectibles",
    "Second Gear Diecast",
    "Autoart Studio Design",
    "Carousel DieCast Legends",
    "Welly Diecast",
];
const SCALES: &[&str] = &[
    "1:10", "1:12", "1:18", "1:24", "1:32", "1:50", "1:72", "1:700",
];
const ROLES: &[&str] = &[
    "[\"ROLE_USER\"]",
    "[\"ROLE_USER\"]",
    "[\"ROLE_USER\"]",
    "[\"ROLE_USER\",\"ROLE_ADMIN\"]",
    "[\"ROLE_SUPER_ADMIN\"]",
    "[]",
];
const NICKNAMES: &[&str] = &[
    "jeanjean",
    "lulu",
    "titi",
    "momo",
    "le \"grand\"",
    "p'tit loup",
    "zaza",
    "nico_31",
    "xXdarkXx",
    "sysadmin",
    "root",
];

// --- Helpers -------------------------------------------------------------

type Rng = ChaCha8Rng;

fn pick<T: Copy>(rng: &mut Rng, items: &[T]) -> T {
    *items.choose(rng).expect("liste non vide")
}

/// Appends an SQL string `'…'`, escaped the way mysqldump does.
fn push_str(buf: &mut Vec<u8>, s: &str) {
    buf.push(b'\'');
    for &b in s.as_bytes() {
        match b {
            b'\'' => buf.extend_from_slice(b"\\'"),
            b'\\' => buf.extend_from_slice(b"\\\\"),
            b'\n' => buf.extend_from_slice(b"\\n"),
            b'\r' => buf.extend_from_slice(b"\\r"),
            0 => buf.extend_from_slice(b"\\0"),
            _ => buf.push(b),
        }
    }
    buf.push(b'\'');
}

fn push_opt_str(buf: &mut Vec<u8>, s: Option<&str>) {
    match s {
        Some(s) => push_str(buf, s),
        None => buf.extend_from_slice(b"NULL"),
    }
}

fn push_num(buf: &mut Vec<u8>, n: impl std::fmt::Display) {
    write!(buf, "{n}").expect("Vec<u8> ne peut pas échouer");
}

fn push_decimal(buf: &mut Vec<u8>, cents: u64) {
    push_num(buf, format_args!("{}.{:02}", cents / 100, cents % 100));
}

fn push_date(buf: &mut Vec<u8>, rng: &mut Rng, year_min: i32, year_max: i32) {
    let y = rng.random_range(year_min..=year_max);
    let m = rng.random_range(1..=12u32);
    let d = rng.random_range(1..=28u32);
    push_str(buf, &format!("{y:04}-{m:02}-{d:02}"));
}

fn push_datetime(buf: &mut Vec<u8>, rng: &mut Rng) {
    let y = rng.random_range(2019..=2026);
    let m = rng.random_range(1..=12u32);
    let d = rng.random_range(1..=28u32);
    let h = rng.random_range(0..24u32);
    let mi = rng.random_range(0..60u32);
    let s = rng.random_range(0..60u32);
    push_str(buf, &format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}"));
}

fn phone(rng: &mut Rng) -> String {
    match rng.random_range(0..4) {
        0 => format!(
            "0{} {:02} {:02} {:02} {:02}",
            rng.random_range(1..=7u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32)
        ),
        1 => format!(
            "+33 {} {:02} {:02} {:02} {:02}",
            rng.random_range(1..=7u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32)
        ),
        2 => format!(
            "0{}{:08}",
            rng.random_range(6..=7u32),
            rng.random_range(0..100_000_000u32)
        ),
        _ => format!(
            "0{}.{:02}.{:02}.{:02}.{:02}",
            rng.random_range(1..=9u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32),
            rng.random_range(0..100u32)
        ),
    }
}

fn address_line2(rng: &mut Rng) -> Option<String> {
    match rng.random_range(0..5) {
        0 => Some(format!("Bât. {}", pick(rng, &["A", "B", "C", "D"]))),
        1 => Some(format!("{}e étage", rng.random_range(1..=12u32))),
        2 => Some(format!("Appt {}", rng.random_range(1..=200u32))),
        _ => None,
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => Some('e'),
            'à' | 'â' | 'ä' => Some('a'),
            'î' | 'ï' => Some('i'),
            'ô' | 'ö' | 'ø' => Some('o'),
            'ù' | 'û' | 'ü' => Some('u'),
            'ç' => Some('c'),
            'É' => Some('e'),
            'Ø' => Some('o'),
            c if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// Product code of the i-th product, e.g. `S18_1042` (deterministic: shared
/// by `products` and `orderdetails`).
fn product_code(i: u64) -> String {
    format!("S{}_{}", 10 + i % 90, 1000 + i)
}

fn hex(rng: &mut Rng, n: usize) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    (0..n)
        .map(|_| HEX[rng.random_range(0..16)] as char)
        .collect()
}

fn bcrypt_like(rng: &mut Rng) -> String {
    const B64: &[u8] = b"./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let body: String = (0..53)
        .map(|_| B64[rng.random_range(0..64)] as char)
        .collect();
    format!("$2y$13${body}")
}

// --- Dump writing ---------------------------------------------------------

/// Counts bytes without writing them (calibration).
struct CountingWriter(u64);

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Row count per table for a given scale factor.
#[derive(Debug, Clone, Copy)]
struct Counts {
    customers: u64,
    employees: u64,
    offices: u64,
    orders: u64,
    max_lines_per_order: u64,
    payments: u64,
    products: u64,
    users: u64,
}

impl Counts {
    fn for_scale(scale: u64) -> Self {
        Counts {
            customers: scale.max(10),
            employees: (scale / 5).max(5),
            offices: 8,
            orders: (scale * 2).max(10),
            max_lines_per_order: 4,
            payments: (scale * 2).max(10),
            products: (scale / 4).max(20),
            users: (scale / 2).max(10),
        }
    }
}

struct Dump<W: Write> {
    out: W,
    rng: Rng,
    counts: Counts,
    buf: Vec<u8>,
}

impl<W: Write> Dump<W> {
    fn header(&mut self) -> Result<()> {
        writeln!(
            self.out,
            "-- MySQL dump 10.13  Distrib 8.0.36, for Linux (x86_64)\n\
             --\n\
             -- Host: replica-01    Database: classicmodels\n\
             -- ------------------------------------------------------\n\
             -- Server version\t8.0.36\n\
             \n\
             /*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;\n\
             /*!40101 SET @OLD_CHARACTER_SET_RESULTS=@@CHARACTER_SET_RESULTS */;\n\
             /*!40101 SET @OLD_COLLATION_CONNECTION=@@COLLATION_CONNECTION */;\n\
             /*!50503 SET NAMES utf8mb4 */;\n\
             /*!40103 SET @OLD_TIME_ZONE=@@TIME_ZONE */;\n\
             /*!40103 SET TIME_ZONE='+00:00' */;\n\
             /*!40014 SET @OLD_UNIQUE_CHECKS=@@UNIQUE_CHECKS, UNIQUE_CHECKS=0 */;\n\
             /*!40014 SET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0 */;\n\
             /*!40101 SET @OLD_SQL_MODE=@@SQL_MODE, SQL_MODE='NO_AUTO_VALUE_ON_ZERO' */;\n\
             /*!40111 SET @OLD_SQL_NOTES=@@SQL_NOTES, SQL_NOTES=0 */;"
        )?;
        Ok(())
    }

    fn footer(&mut self) -> Result<()> {
        writeln!(
            self.out,
            "/*!40103 SET TIME_ZONE=@OLD_TIME_ZONE */;\n\
             \n\
             /*!40101 SET SQL_MODE=@OLD_SQL_MODE */;\n\
             /*!40014 SET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS */;\n\
             /*!40014 SET UNIQUE_CHECKS=@OLD_UNIQUE_CHECKS */;\n\
             /*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;\n\
             /*!40101 SET CHARACTER_SET_RESULTS=@OLD_CHARACTER_SET_RESULTS */;\n\
             /*!40101 SET COLLATION_CONNECTION=@OLD_COLLATION_CONNECTION */;\n\
             /*!40111 SET SQL_NOTES=@OLD_SQL_NOTES */;\n\
             \n\
             -- Dump completed on 2026-09-15 18:19:00"
        )?;
        Ok(())
    }

    fn schema(&mut self, table: &str, body: &str) -> Result<()> {
        writeln!(
            self.out,
            "\n--\n-- Table structure for table `{table}`\n--\n\n\
             DROP TABLE IF EXISTS `{table}`;\n\
             /*!40101 SET @saved_cs_client     = @@character_set_client */;\n\
             /*!50503 SET character_set_client = utf8mb4 */;\n\
             CREATE TABLE `{table}` (\n{body}\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;\n\
             /*!40101 SET character_set_client = @saved_cs_client */;\n\
             \n--\n-- Dumping data for table `{table}`\n--\n\n\
             LOCK TABLES `{table}` WRITE;\n\
             /*!40000 ALTER TABLE `{table}` DISABLE KEYS */;"
        )?;
        Ok(())
    }

    fn end_table(&mut self, table: &str) -> Result<()> {
        writeln!(
            self.out,
            "/*!40000 ALTER TABLE `{table}` ENABLE KEYS */;\nUNLOCK TABLES;"
        )?;
        Ok(())
    }

    /// Writes `n` rows produced by `row` as extended `INSERT`s of about 1 MiB.
    fn rows(
        &mut self,
        table: &str,
        n: u64,
        mut row: impl FnMut(&mut Rng, u64, &mut Vec<u8>),
    ) -> Result<()> {
        let prefix = format!("INSERT INTO `{table}` VALUES ");
        let mut open = false;
        for i in 0..n {
            if !open {
                self.buf.extend_from_slice(prefix.as_bytes());
                open = true;
            } else {
                self.buf.push(b',');
            }
            self.buf.push(b'(');
            row(&mut self.rng, i, &mut self.buf);
            self.buf.push(b')');
            if self.buf.len() >= INSERT_MAX_BYTES {
                self.buf.extend_from_slice(b";\n");
                self.out.write_all(&self.buf)?;
                self.buf.clear();
                open = false;
            }
        }
        if open {
            self.buf.extend_from_slice(b";\n");
            self.out.write_all(&self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }

    fn write_all_tables(&mut self) -> Result<()> {
        let c = self.counts;
        self.header()?;

        // customers ---------------------------------------------------------
        self.schema(
            "customers",
            "  `customerNumber` int NOT NULL,\n\
             \x20 `customerName` varchar(50) NOT NULL,\n\
             \x20 `contactLastName` varchar(50) NOT NULL,\n\
             \x20 `contactFirstName` varchar(50) NOT NULL,\n\
             \x20 `phone` varchar(50) NOT NULL,\n\
             \x20 `addressLine1` varchar(50) NOT NULL,\n\
             \x20 `addressLine2` varchar(50) DEFAULT NULL,\n\
             \x20 `city` varchar(50) NOT NULL,\n\
             \x20 `state` varchar(50) DEFAULT NULL,\n\
             \x20 `postalCode` varchar(15) DEFAULT NULL,\n\
             \x20 `country` varchar(50) NOT NULL,\n\
             \x20 `salesRepEmployeeNumber` int DEFAULT NULL,\n\
             \x20 `creditLimit` decimal(10,2) DEFAULT NULL,\n\
             \x20 PRIMARY KEY (`customerNumber`),\n\
             \x20 KEY `salesRepEmployeeNumber` (`salesRepEmployeeNumber`),\n\
             \x20 CONSTRAINT `customers_ibfk_1` FOREIGN KEY (`salesRepEmployeeNumber`) REFERENCES `employees` (`employeeNumber`)",
        )?;
        self.rows("customers", c.customers, |rng, i, b| {
            push_num(b, 103 + i);
            b.push(b',');
            let name = format!("{} {}", pick(rng, COMPANY_A), pick(rng, COMPANY_B));
            push_str(b, &name);
            b.push(b',');
            push_str(b, pick(rng, LAST_NAMES));
            b.push(b',');
            push_str(b, pick(rng, FIRST_NAMES));
            b.push(b',');
            push_str(b, &phone(rng));
            b.push(b',');
            let street = format!("{} {}", rng.random_range(1..=250u32), pick(rng, STREETS));
            push_str(b, &street);
            b.push(b',');
            push_opt_str(b, address_line2(rng).as_deref());
            b.push(b',');
            let (city, cp, state) = pick(rng, CITIES);
            push_str(b, city);
            b.push(b',');
            if rng.random_bool(0.7) {
                push_str(b, state);
            } else {
                b.extend_from_slice(b"NULL");
            }
            b.push(b',');
            push_str(b, cp);
            b.push(b',');
            push_str(b, pick(rng, COUNTRIES));
            b.push(b',');
            if rng.random_bool(0.85) {
                push_num(b, 1000 + rng.random_range(0..c.employees));
            } else {
                b.extend_from_slice(b"NULL");
            }
            b.push(b',');
            push_decimal(b, rng.random_range(0..20_000_000u64));
        })?;
        self.end_table("customers")?;

        // employees ---------------------------------------------------------
        self.schema(
            "employees",
            "  `employeeNumber` int NOT NULL,\n\
             \x20 `lastName` varchar(50) NOT NULL,\n\
             \x20 `firstName` varchar(50) NOT NULL,\n\
             \x20 `extension` varchar(10) NOT NULL,\n\
             \x20 `email` varchar(100) NOT NULL,\n\
             \x20 `officeCode` varchar(10) NOT NULL,\n\
             \x20 `reportsTo` int DEFAULT NULL,\n\
             \x20 `jobTitle` varchar(50) NOT NULL,\n\
             \x20 PRIMARY KEY (`employeeNumber`),\n\
             \x20 KEY `reportsTo` (`reportsTo`),\n\
             \x20 KEY `officeCode` (`officeCode`),\n\
             \x20 CONSTRAINT `employees_ibfk_1` FOREIGN KEY (`reportsTo`) REFERENCES `employees` (`employeeNumber`),\n\
             \x20 CONSTRAINT `employees_ibfk_2` FOREIGN KEY (`officeCode`) REFERENCES `offices` (`officeCode`)",
        )?;
        self.rows("employees", c.employees, |rng, i, b| {
            push_num(b, 1000 + i);
            b.push(b',');
            let last = pick(rng, LAST_NAMES);
            let first = pick(rng, FIRST_NAMES);
            push_str(b, last);
            b.push(b',');
            push_str(b, first);
            b.push(b',');
            push_str(b, &format!("x{}", rng.random_range(1000..10000u32)));
            b.push(b',');
            push_str(
                b,
                &format!("{}.{}{}@classicmodels.com", slug(first), slug(last), i),
            );
            b.push(b',');
            push_num(b, 1 + rng.random_range(0..c.offices));
            b.push(b',');
            if i == 0 {
                b.extend_from_slice(b"NULL");
            } else {
                push_num(b, 1000 + rng.random_range(0..i));
            }
            b.push(b',');
            push_str(b, pick(rng, JOB_TITLES));
        })?;
        self.end_table("employees")?;

        // offices -----------------------------------------------------------
        self.schema(
            "offices",
            "  `officeCode` varchar(10) NOT NULL,\n\
             \x20 `city` varchar(50) NOT NULL,\n\
             \x20 `phone` varchar(50) NOT NULL,\n\
             \x20 `addressLine1` varchar(50) NOT NULL,\n\
             \x20 `addressLine2` varchar(50) DEFAULT NULL,\n\
             \x20 `state` varchar(50) DEFAULT NULL,\n\
             \x20 `country` varchar(50) NOT NULL,\n\
             \x20 `postalCode` varchar(15) NOT NULL,\n\
             \x20 `territory` varchar(10) NOT NULL,\n\
             \x20 PRIMARY KEY (`officeCode`)",
        )?;
        self.rows("offices", c.offices, |rng, i, b| {
            push_str(b, &(i + 1).to_string());
            b.push(b',');
            let (city, cp, state) = CITIES[i as usize % CITIES.len()];
            push_str(b, city);
            b.push(b',');
            push_str(b, &phone(rng));
            b.push(b',');
            push_str(
                b,
                &format!("{} {}", rng.random_range(1..=99u32), pick(rng, STREETS)),
            );
            b.push(b',');
            push_opt_str(b, address_line2(rng).as_deref());
            b.push(b',');
            push_str(b, state);
            b.push(b',');
            push_str(b, "France");
            b.push(b',');
            push_str(b, cp);
            b.push(b',');
            push_str(b, "EMEA");
        })?;
        self.end_table("offices")?;

        // productlines / products ------------------------------------------
        self.schema(
            "productlines",
            "  `productLine` varchar(50) NOT NULL,\n\
             \x20 `textDescription` varchar(4000) DEFAULT NULL,\n\
             \x20 `htmlDescription` mediumtext,\n\
             \x20 `image` mediumblob,\n\
             \x20 PRIMARY KEY (`productLine`)",
        )?;
        self.rows("productlines", PRODUCT_LINES.len() as u64, |_, i, b| {
            let (line, desc) = PRODUCT_LINES[i as usize];
            push_str(b, line);
            b.push(b',');
            push_str(b, desc);
            b.extend_from_slice(b",NULL,NULL");
        })?;
        self.end_table("productlines")?;

        self.schema(
            "products",
            "  `productCode` varchar(15) NOT NULL,\n\
             \x20 `productName` varchar(70) NOT NULL,\n\
             \x20 `productLine` varchar(50) NOT NULL,\n\
             \x20 `productScale` varchar(10) NOT NULL,\n\
             \x20 `productVendor` varchar(50) NOT NULL,\n\
             \x20 `productDescription` text NOT NULL,\n\
             \x20 `quantityInStock` smallint NOT NULL,\n\
             \x20 `buyPrice` decimal(10,2) NOT NULL,\n\
             \x20 `MSRP` decimal(10,2) NOT NULL,\n\
             \x20 PRIMARY KEY (`productCode`),\n\
             \x20 KEY `productLine` (`productLine`),\n\
             \x20 CONSTRAINT `products_ibfk_1` FOREIGN KEY (`productLine`) REFERENCES `productlines` (`productLine`)",
        )?;
        self.rows("products", c.products, |rng, i, b| {
            push_str(b, &product_code(i));
            b.push(b',');
            push_str(
                b,
                &format!("{} {}", pick(rng, PRODUCT_ADJ), pick(rng, PRODUCT_NOUN)),
            );
            b.push(b',');
            push_str(b, pick(rng, PRODUCT_LINES).0);
            b.push(b',');
            push_str(b, pick(rng, SCALES));
            b.push(b',');
            push_str(b, pick(rng, VENDORS));
            b.push(b',');
            push_str(
                b,
                "This replica features working steering system, opening doors, detailed \
                 interior, working headlights and more. Peinture métallisée « premium », \
                 livrée dans un coffret d'origine.",
            );
            b.push(b',');
            push_num(b, rng.random_range(0..10_000u32));
            b.push(b',');
            let buy = rng.random_range(1_500..12_000u64);
            push_decimal(b, buy);
            b.push(b',');
            push_decimal(b, buy * 17 / 10);
        })?;
        self.end_table("products")?;

        // orders / orderdetails --------------------------------------------
        self.schema(
            "orders",
            "  `orderNumber` int NOT NULL,\n\
             \x20 `orderDate` date NOT NULL,\n\
             \x20 `requiredDate` date NOT NULL,\n\
             \x20 `shippedDate` date DEFAULT NULL,\n\
             \x20 `status` varchar(15) NOT NULL,\n\
             \x20 `comments` text,\n\
             \x20 `customerNumber` int NOT NULL,\n\
             \x20 PRIMARY KEY (`orderNumber`),\n\
             \x20 KEY `customerNumber` (`customerNumber`),\n\
             \x20 CONSTRAINT `orders_ibfk_1` FOREIGN KEY (`customerNumber`) REFERENCES `customers` (`customerNumber`)",
        )?;
        self.rows("orders", c.orders, |rng, i, b| {
            push_num(b, 10_100 + i);
            b.push(b',');
            push_date(b, rng, 2020, 2026);
            b.push(b',');
            push_date(b, rng, 2020, 2026);
            b.push(b',');
            if rng.random_bool(0.8) {
                push_date(b, rng, 2020, 2026);
            } else {
                b.extend_from_slice(b"NULL");
            }
            b.push(b',');
            push_str(b, pick(rng, ORDER_STATUS));
            b.push(b',');
            push_opt_str(b, pick(rng, COMMENTS));
            b.push(b',');
            push_num(b, 103 + rng.random_range(0..c.customers));
        })?;
        self.end_table("orders")?;

        self.schema(
            "orderdetails",
            "  `orderNumber` int NOT NULL,\n\
             \x20 `productCode` varchar(15) NOT NULL,\n\
             \x20 `quantityOrdered` int NOT NULL,\n\
             \x20 `priceEach` decimal(10,2) NOT NULL,\n\
             \x20 `orderLineNumber` smallint NOT NULL,\n\
             \x20 PRIMARY KEY (`orderNumber`,`productCode`),\n\
             \x20 KEY `productCode` (`productCode`),\n\
             \x20 CONSTRAINT `orderdetails_ibfk_1` FOREIGN KEY (`orderNumber`) REFERENCES `orders` (`orderNumber`),\n\
             \x20 CONSTRAINT `orderdetails_ibfk_2` FOREIGN KEY (`productCode`) REFERENCES `products` (`productCode`)",
        )?;
        // Each order has `max_lines_per_order` lines, on distinct (composite
        // primary key) and existing (foreign key) products.
        let lines = c.orders * c.max_lines_per_order;
        self.rows("orderdetails", lines, |rng, i, b| {
            let order = i / c.max_lines_per_order;
            let line = i % c.max_lines_per_order;
            push_num(b, 10_100 + order);
            b.push(b',');
            let product = (order * 7 + line * 13) % c.products;
            push_str(b, &product_code(product));
            b.push(b',');
            push_num(b, rng.random_range(1..=100u32));
            b.push(b',');
            push_decimal(b, rng.random_range(1_500..20_000u64));
            b.push(b',');
            push_num(b, line + 1);
        })?;
        self.end_table("orderdetails")?;

        // payments ----------------------------------------------------------
        self.schema(
            "payments",
            "  `customerNumber` int NOT NULL,\n\
             \x20 `checkNumber` varchar(50) NOT NULL,\n\
             \x20 `paymentDate` date NOT NULL,\n\
             \x20 `amount` decimal(10,2) NOT NULL,\n\
             \x20 PRIMARY KEY (`customerNumber`,`checkNumber`),\n\
             \x20 CONSTRAINT `payments_ibfk_1` FOREIGN KEY (`customerNumber`) REFERENCES `customers` (`customerNumber`)",
        )?;
        self.rows("payments", c.payments, |rng, i, b| {
            push_num(b, 103 + rng.random_range(0..c.customers));
            b.push(b',');
            let a = (b'A' + rng.random_range(0..26u8)) as char;
            let z = (b'A' + rng.random_range(0..26u8)) as char;
            push_str(b, &format!("{a}{z}{i:06}"));
            b.push(b',');
            push_date(b, rng, 2020, 2026);
            b.push(b',');
            push_decimal(b, rng.random_range(1_000..10_000_000u64));
        })?;
        self.end_table("payments")?;

        // user --------------------------------------------------------------
        self.schema(
            "user",
            "  `id` int NOT NULL AUTO_INCREMENT,\n\
             \x20 `email` varchar(180) NOT NULL,\n\
             \x20 `first_name` varchar(100) NOT NULL,\n\
             \x20 `last_name` varchar(100) NOT NULL,\n\
             \x20 `phone` varchar(20) DEFAULT NULL,\n\
             \x20 `birth_date` date DEFAULT NULL,\n\
             \x20 `password` varchar(255) NOT NULL,\n\
             \x20 `api_token` varchar(64) DEFAULT NULL,\n\
             \x20 `nickname` varchar(50) DEFAULT NULL,\n\
             \x20 `roles` json NOT NULL,\n\
             \x20 `created_at` datetime NOT NULL,\n\
             \x20 PRIMARY KEY (`id`),\n\
             \x20 UNIQUE KEY `UNIQ_8D93D649E7927C74` (`email`)",
        )?;
        self.rows("user", c.users, |rng, i, b| {
            push_num(b, i + 1);
            b.push(b',');
            let first = pick(rng, FIRST_NAMES);
            let last = pick(rng, LAST_NAMES);
            let email = match rng.random_range(0..3) {
                0 => format!("{}.{}{}@{}", slug(first), slug(last), i, pick(rng, DOMAINS)),
                1 => format!("{}{}@{}", slug(first), i, pick(rng, DOMAINS)),
                _ => format!("{}{}@{}", slug(last), i, pick(rng, DOMAINS)),
            };
            push_str(b, &email);
            b.push(b',');
            push_str(b, first);
            b.push(b',');
            push_str(b, last);
            b.push(b',');
            if rng.random_bool(0.8) {
                push_str(b, &phone(rng));
            } else {
                b.extend_from_slice(b"NULL");
            }
            b.push(b',');
            if rng.random_bool(0.6) {
                push_date(b, rng, 1940, 2008);
            } else {
                b.extend_from_slice(b"NULL");
            }
            b.push(b',');
            push_str(b, &bcrypt_like(rng));
            b.push(b',');
            if rng.random_bool(0.5) {
                push_str(b, &format!("tok_{}", hex(rng, 32)));
            } else {
                b.extend_from_slice(b"NULL");
            }
            b.push(b',');
            if rng.random_bool(0.4) {
                push_str(b, pick(rng, NICKNAMES));
            } else {
                b.extend_from_slice(b"NULL");
            }
            b.push(b',');
            push_str(b, pick(rng, ROLES));
            b.push(b',');
            push_datetime(b, rng);
        })?;
        self.end_table("user")?;

        self.footer()?;
        self.out.flush()?;
        Ok(())
    }
}

fn generate<W: Write>(out: W, seed: u64, scale: u64) -> Result<()> {
    let mut dump = Dump {
        out,
        rng: Rng::seed_from_u64(seed),
        counts: Counts::for_scale(scale),
        buf: Vec::with_capacity(INSERT_MAX_BYTES + 4096),
    };
    dump.write_all_tables()
}

/// Output size for a scale factor, without writing anything.
fn measure(seed: u64, scale: u64) -> Result<u64> {
    let mut counter = CountingWriter(0);
    generate(&mut counter, seed, scale)?;
    Ok(counter.0)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let target = parse_size(&args.size)?;

    // Calibration: size is affine in `scale` (y = a + b·scale).
    let (s1, s2) = (2_000u64, 4_000u64);
    let (y1, y2) = (measure(args.seed, s1)?, measure(args.seed, s2)?);
    let per_unit = (y2 - y1) as f64 / (s2 - s1) as f64;
    let base = y1 as f64 - per_unit * s1 as f64;
    let scale = ((target as f64 - base) / per_unit).max(1.0) as u64;
    let counts = Counts::for_scale(scale);

    if let Some(dir) = args.out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    }
    let file =
        File::create(&args.out).with_context(|| format!("création de {}", args.out.display()))?;
    eprintln!(
        "génération de {} (~{} Mio, seed {}) : {} customers, {} employees, {} products, \
         {} orders, {} orderdetails, {} payments, {} users…",
        args.out.display(),
        target >> 20,
        args.seed,
        counts.customers,
        counts.employees,
        counts.products,
        counts.orders,
        counts.orders * counts.max_lines_per_order,
        counts.payments,
        counts.users
    );
    let started = std::time::Instant::now();
    generate(BufWriter::with_capacity(4 << 20, file), args.seed, scale)?;
    let len = std::fs::metadata(&args.out)?.len();
    eprintln!(
        "✔ {} écrit : {:.1} Mio en {:.1}s",
        args.out.display(),
        len as f64 / (1u64 << 20) as f64,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
