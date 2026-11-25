//! App assets: our own files first, then gpui-kit's icon catalog.
//!
//! Database logos under `db-icons/` come from Simple Icons (CC0); see
//! `crates/thaan-app/assets/db-icons/SOURCES.md`. They are embedded with `include_bytes!`, so
//! they cost a few KB and no disk reads at runtime.

use gpui_kit::*;
use std::borrow::Cow;

/// `(asset path, bytes)` for every file this app ships itself.
const OWN: &[(&str, &[u8])] = &[
    (
        "db-icons/postgresql.svg",
        include_bytes!("../assets/db-icons/postgresql.svg"),
    ),
    (
        "db-icons/mysql.svg",
        include_bytes!("../assets/db-icons/mysql.svg"),
    ),
    (
        "db-icons/mariadb.svg",
        include_bytes!("../assets/db-icons/mariadb.svg"),
    ),
    (
        "db-icons/sqlite.svg",
        include_bytes!("../assets/db-icons/sqlite.svg"),
    ),
    (
        "db-icons/mongodb.svg",
        include_bytes!("../assets/db-icons/mongodb.svg"),
    ),
    (
        "db-icons/redis.svg",
        include_bytes!("../assets/db-icons/redis.svg"),
    ),
    (
        "db-icons/mqtt.svg",
        include_bytes!("../assets/db-icons/mqtt.svg"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = OWN.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = OWN
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect();
        out.extend(gpui_kit::assets::AllAssets.list(path)?);
        Ok(out)
    }
}

/// The logo asset for a driver id, if we ship one. MySQL and MariaDB share one driver,
/// which shows the MySQL logo.
pub fn db_logo_path(driver_id: &str) -> Option<&'static str> {
    Some(match driver_id {
        "postgres" => "db-icons/postgresql.svg",
        "mysql" => "db-icons/mysql.svg",
        "mariadb" => "db-icons/mariadb.svg",
        "sqlite" => "db-icons/sqlite.svg",
        "mongodb" => "db-icons/mongodb.svg",
        "redis" => "db-icons/redis.svg",
        "mqtt" => "db-icons/mqtt.svg",
        _ => return None,
    })
}
