use bdo_rs::BDO;
use serde::{Deserialize, Serialize};
use sessionless::hex::IntoHex;
use sessionless::secp256k1::SecretKey;
use sessionless::Sessionless;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::Manager;

const MAX_CARDS: usize = 4;
const BDO_HASH: &str = "bizbuz-card";

// ── Palette ──────────────────────────────────────────────────────────────────
//
// HomeVentory dark, matching the app's own dark mode in style.css (the
// `prefers-color-scheme: dark` block: #1a1a1a ground, glacier-white text) and
// Linkitylink's published card, so the two apps' web cards are the same.
//
// Evergreen stays for FILLED surfaces — the avatar circle and the button —
// but not for text or thin strokes on the ground: #2E5E4E on #1a1a1a is about
// 2.3:1, too low to read. Those use soft mint instead, from the same palette.
//
// PALETTE_* is also sent to savage on publish (see publish_card), which
// themes the page chrome around the card — savage otherwise falls back to
// that same old BizBuz scheme for every app it serves.
const PALETTE_BG: &str = "#1a1a1a";        // app dark-mode ground
const PALETTE_GREEN: &str = "#2E5E4E";     // deep evergreen
const PALETTE_ACCENT: &str = "#4FA3F7";    // signal blue
const PALETTE_MINT: &str = "#AEE1D6";      // soft mint — accent text on dark
/// Text drawn ON an evergreen fill (avatar initials, button label). Was
/// written as {BG}, which only worked while BG happened to be light.
const PALETTE_ON_GREEN: &str = "#F7F9FA";  // glacier white
/// Glacier white (#F7F9FA) as rgb components — the ink on the dark ground. The published SVGs need the ink colour at
/// several opacities, and a Rust raw string can't carry an inline hex literal
/// (`r#"..."#` terminates at the first `"#`), so these are interpolated as
/// rgba(...) instead of written as hex.
const PALETTE_INK_RGB: &str = "247,249,250";

// ── Which base this install talks to ─────────────────────────────────────────
//
// Every base is an allyabase behind path-based nginx routing: TLS terminated
// on 443, /<service>/ proxied to that service's local port. One hostname, one
// certificate, everything over real HTTPS — which is also what keeps iOS ATS
// happy, since the services themselves speak plain HTTP.
//
// Which hostname, though, is per-install rather than compiled in. The state on
// a user's FIRST card picks their base — <state>.8as.world — and that choice
// is then pinned for the life of the install (see `established_base`).
//
// It has to be pinned, not recomputed, because BDO mints its own server-side
// uuid on create_user: a uuid minted against one base 404s an update_bdo call
// against another. Letting the base drift after a card is published would
// silently orphan it. For the same reason every published uuid is stored
// per-env in `bdo_uuid_by_env`, keyed by `env_key_for` below.

/// Where installs that already published against the old single base stay.
/// Their uuids only resolve there, so an upgrade must not move them.
const LEGACY_BASE_HOST: &str = "dev.8as.world";

/// Used when no state has been chosen yet, or the chosen state somehow isn't
/// one of the fifty. Not a per-state base.
const FALLBACK_BASE_HOST: &str = "prod.8as.world";

fn base_host_to_env_key(host: &str) -> String {
    host.replace('.', "-")
}

fn bdo_url_for(host: &str) -> String {
    format!("https://{host}/bdo/")
}

fn savage_url_for(host: &str) -> String {
    format!("https://{host}/savage/")
}

/// The fifty states, as two-letter USPS codes. Deliberately fifty and no
/// more: one base per state is the whole design. DC, PR, and the other
/// territories have no code here and therefore no base — a card claiming one
/// falls back to FALLBACK_BASE_HOST rather than inventing a 51st host.
const STATE_CODES: &[&str] = &[
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA",
    "HI", "ID", "IL", "IN", "IA", "KS", "KY", "LA", "ME", "MD",
    "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ",
    "NM", "NY", "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC",
    "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV", "WI", "WY",
];

fn is_valid_state(code: &str) -> bool {
    let upper = code.trim().to_uppercase();
    STATE_CODES.iter().any(|s| *s == upper)
}

/// `CA` → `ca.8as.world`. Anything not one of the fifty → the fallback base.
fn base_host_for_state(state: Option<&str>) -> String {
    match state {
        Some(s) if is_valid_state(s) => format!("{}.8as.world", s.trim().to_lowercase()),
        _ => FALLBACK_BASE_HOST.to_string(),
    }
}

// ── Base pinning ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BaseRecord {
    host: String,
    /// What established it — the state code, or a marker for the two cases
    /// that don't come from a state. Diagnostic only; `host` is the authority.
    established_by: String,
}

fn base_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("base.json"))
}

fn read_base(app: &tauri::AppHandle) -> Option<BaseRecord> {
    let path = base_path(app).ok()?;
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn write_base(app: &tauri::AppHandle, record: &BaseRecord) -> Result<(), String> {
    let path = base_path(app)?;
    let json = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

/// True if this install published anything before per-state bases existed.
/// Those uuids live on LEGACY_BASE_HOST and resolve nowhere else, so such an
/// install has to stay there — the alternative is orphaning live share links.
fn has_legacy_publishes(app: &tauri::AppHandle) -> bool {
    let legacy_key = base_host_to_env_key(LEGACY_BASE_HOST);
    read_cards(app)
        .map(|store| {
            store.cards.iter().any(|c| c.bdo_uuid_by_env.contains_key(&legacy_key))
        })
        .unwrap_or(false)
}

/// The base this install uses, pinning it on first call if it isn't pinned yet.
///
/// Order matters. An existing install that already published is pinned to the
/// legacy base before any state is consulted, so upgrading never moves a user
/// whose cards are already live somewhere.
fn established_base(app: &tauri::AppHandle) -> String {
    if let Some(record) = read_base(app) {
        return record.host;
    }

    let record = if has_legacy_publishes(app) {
        BaseRecord {
            host: LEGACY_BASE_HOST.to_string(),
            established_by: "legacy-publish".to_string(),
        }
    } else {
        // Nothing published yet and nothing pinned: derive from the first card
        // that names a state. Callers normally pin explicitly via
        // `establish_base_from_state` on save; this is the fallback for a
        // publish that somehow precedes it.
        let state = read_cards(app)
            .ok()
            .and_then(|store| store.cards.iter().find_map(|c| c.state.clone()));

        match state {
            Some(s) if is_valid_state(&s) => BaseRecord {
                host: base_host_for_state(Some(&s)),
                established_by: s.trim().to_uppercase(),
            },
            _ => BaseRecord {
                host: FALLBACK_BASE_HOST.to_string(),
                established_by: "no-state".to_string(),
            },
        }
    };

    // A write failure here is not fatal: the same inputs recompute the same
    // host next time. It only means the pin isn't durable yet.
    let _ = write_base(app, &record);
    record.host
}

/// Pins the base from a card's state, if nothing is pinned yet. Called when a
/// card is saved, so the FIRST card's state is what establishes the base —
/// later cards, in other states, don't move an install that's already pinned.
fn establish_base_from_state(app: &tauri::AppHandle, state: Option<&str>) {
    if read_base(app).is_some() || has_legacy_publishes(app) {
        return;
    }
    let Some(state) = state.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    if !is_valid_state(state) {
        return;
    }
    let record = BaseRecord {
        host: base_host_for_state(Some(state)),
        established_by: state.to_uppercase(),
    };
    let _ = write_base(app, &record);
}

// ── Categories ───────────────────────────────────────────────────────────────
//
// Same slug/label list as idothis's own CATEGORIES (the "what does this
// business do" taxonomy every app in this bundle should agree on) — kept as
// its own copy here rather than a shared crate, matching this ecosystem's
// existing per-app-copy convention (see jobs.rs/locations.rs in Gettit/
// letemcook for the same pattern). Keep in sync by hand if either list
// changes.

// (Category selection lives on the shared Canonical Profile now — idothis
// owns the taxonomy, and BizBuz no longer has its own copy or a
// `<select>` for it. Any "what kind of business is this" tagging is
// expressed via the free-form canonical fields the user fills in for
// cross-app use.)

// ── Data types ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Social {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instagram: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiktok: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub youtube: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facebook: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linkedin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codeberg: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Two-letter USPS state code. Distinct from the free-form `location`
    /// above, which stays unstructured ("The Cosmos" is a valid location).
    /// The FIRST card to carry one picks this install's base — see
    /// `established_base`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(default)]
    pub social: Social,
    /// Base64-encoded JPEG (no `data:` prefix), resized/cropped client-side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo: Option<String>,
    /// BDO identity uuid this card is published under, per environment
    /// (see `base_host_to_env_key`) — a fresh base has no entry until first publish.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub bdo_uuid_by_env: HashMap<String, String>,
    /// Full pre-signed savage URL — hosted by allyabase's own infrastructure
    /// (not a bizbuz-run server), rendering the SVG this card was published
    /// with as a live webpage. Computed locally at publish time (signing is
    /// local, no round-trip) and doesn't expire, so it's safe to treat as a
    /// permanent link rather than something that needs refreshing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CardsStore {
    cards: Vec<Profile>,
}

// ── Storage ──────────────────────────────────────────────────────────────────

fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn cards_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("cards.json"))
}

/// Pre-multi-card storage location — a single `Profile` written directly as
/// JSON (no wrapping `cards` array). Migrated into cards.json on first load
/// if a user already had one card from before this feature existed.
fn legacy_profile_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("profile.json"))
}

fn write_cards(app: &tauri::AppHandle, store: &CardsStore) -> Result<(), String> {
    let path = cards_path(app)?;
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

fn read_cards(app: &tauri::AppHandle) -> Result<CardsStore, String> {
    let path = cards_path(app)?;
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).map_err(|e| e.to_string()),
        Err(_) => {
            // Nothing at cards.json yet — check for a pre-multi-card profile.json
            // and migrate it in as this user's first card.
            match fs::read_to_string(legacy_profile_path(app)?) {
                Ok(contents) => {
                    let mut profile: Profile =
                        serde_json::from_str(&contents).map_err(|e| e.to_string())?;
                    if profile.id.is_empty() {
                        profile.id = new_id();
                    }
                    let store = CardsStore { cards: vec![profile] };
                    write_cards(app, &store)?;
                    Ok(store)
                }
                Err(_) => Ok(CardsStore::default()),
            }
        }
    }
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Nanosecond timestamp + a monotonic in-process counter, so two cards
/// created back-to-back never collide even if the clock's actual resolution
/// is coarser than a nanosecond (a plain millisecond timestamp collided
/// under exactly this scenario during testing).
fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", nanos, counter)
}

fn unix_now_ms_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

// ── BDO publishing ───────────────────────────────────────────────────────────
//
// BDO's public storage is keyed by pubKey — one identity, one publicly
// retrievable slot, overwritten on every public write regardless of the
// `hash` used (verified directly against allyabase's db.js). So each card
// that gets published needs its own sessionless keypair, not one shared
// device identity. Keys live in their own file, separate from cards.json.

#[derive(Debug, Serialize, Deserialize, Clone)]
struct BdoKeypair {
    private_key_hex: String,
    pub_key_hex: String,
}

fn bdo_keys_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("bdo_keys.json"))
}

fn read_bdo_keys(app: &tauri::AppHandle) -> HashMap<String, BdoKeypair> {
    let path = match bdo_keys_path(app) {
        Ok(p) => p,
        Err(_) => return HashMap::new(),
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_bdo_keys(app: &tauri::AppHandle, keys: &HashMap<String, BdoKeypair>) -> Result<(), String> {
    let path = bdo_keys_path(app)?;
    let json = serde_json::to_string_pretty(keys).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

fn sessionless_from_hex(priv_key_hex: &str) -> Result<Sessionless, String> {
    let bytes = hex::decode(priv_key_hex).map_err(|e| e.to_string())?;
    let secret_key = SecretKey::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(Sessionless::from_private_key(secret_key))
}

/// This key's BDO identity if it already has one, without creating it.
///
/// Deletion must never mint a keypair: a card that was never published has no
/// remote record, and creating a key just to "delete" one would be pointless
/// churn. Returns None when the key has never been used.
fn existing_bdo_sessionless(app: &tauri::AppHandle, key: &str) -> Option<Sessionless> {
    read_bdo_keys(app)
        .get(key)
        .and_then(|k| sessionless_from_hex(&k.private_key_hex).ok())
}

/// Unpublishes every remote record a key has, across every base it published
/// to, and reports which bases failed.
///
/// Returns the list of env keys that could NOT be deleted. An empty list means
/// the remote is clean and the local copy is safe to drop. Callers must not
/// delete local state while this is non-empty — `bdo_uuid_by_env` and the
/// keypair are the only way back to those records, so discarding them leaves
/// the user's contact details published with no way for anyone, including us,
/// to ever take them down.
async fn unpublish_everywhere(
    app: &tauri::AppHandle,
    key: &str,
    hash: &str,
    uuids_by_env: &HashMap<String, String>,
) -> Vec<String> {
    if existing_bdo_sessionless(app, key).is_none() {
        // Never published; nothing remote to remove.
        return Vec::new();
    }

    let mut failed = Vec::new();
    for (env_key, uuid) in uuids_by_env {
        // Sessionless isn't Clone, and BDO::new takes ownership, so rebuild it
        // per base from the stored key rather than holding one across the loop.
        let Some(sessionless) = existing_bdo_sessionless(app, key) else {
            failed.push(env_key.clone());
            continue;
        };

        // env keys are host names with dots swapped for dashes, so this
        // reverses the mapping to reach the base that actually holds the
        // record — which may not be the base this install now publishes to.
        let host = env_key.replace('-', ".");
        let client = BDO::new(Some(bdo_url_for(&host)), Some(sessionless));

        if client.delete_user(uuid, hash).await.is_err() {
            failed.push(env_key.clone());
        }
    }
    failed
}

/// Returns this card's BDO identity, generating and persisting a fresh
/// keypair the first time a given card is published.
fn load_or_create_bdo_sessionless(app: &tauri::AppHandle, card_id: &str) -> Result<Sessionless, String> {
    let mut keys = read_bdo_keys(app);
    if let Some(existing) = keys.get(card_id) {
        return sessionless_from_hex(&existing.private_key_hex);
    }

    let session = Sessionless::new();
    keys.insert(
        card_id.to_string(),
        BdoKeypair {
            private_key_hex: session.private_key().to_hex(),
            pub_key_hex: session.public_key().to_hex(),
        },
    );
    write_bdo_keys(app, &keys)?;
    Ok(session)
}

// ── SVG card rendering ───────────────────────────────────────────────────────
//
// savage only renders whatever `svg` property is already present on a
// published BDO's data — there's no server-side rendering step, so the app
// builds the visual card itself and publishes it alongside the structured
// fields.

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn get_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if initials.is_empty() {
        "?".to_string()
    } else {
        initials
    }
}

/// Greedy word-wrap at `max_chars` per line, capped at `max_lines`. If the
/// text doesn't fit, the last line is trimmed and given a trailing ellipsis
/// rather than silently dropping the overflow.
fn wrap_text(text: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut i = 0;

    while i < words.len() && lines.len() < max_lines {
        let word = words[i];
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if candidate.chars().count() > max_chars && !current.is_empty() {
            lines.push(current.clone());
            current.clear();
        } else {
            current = candidate;
            i += 1;
        }
    }

    let truncated = i < words.len();
    if !current.is_empty() {
        lines.push(current);
    }

    if truncated {
        if let Some(last) = lines.last_mut() {
            while last.chars().count() + 1 > max_chars && !last.is_empty() {
                last.pop();
            }
            last.push('…');
        }
    }

    lines
}

/// Renders a `Profile` as a self-contained SVG business card. Mirrors the
/// dark/green/purple visual language already used in server.js's HTML card
/// and the app's own style.css.
fn render_card_svg(profile: &Profile) -> String {
    const WIDTH: u32 = 400;
    const BG: &str = PALETTE_BG;
    const GREEN: &str = PALETTE_GREEN;
    const PURPLE: &str = PALETTE_ACCENT;

    let name = profile.name.clone().unwrap_or_else(|| "".to_string());
    // Name baseline. The avatar's bottom edge is cy + r = 170, and a 26px bold
    // name's cap height reaches ~19px above its baseline — at 190 that left
    // about a pixel between them. 216 gives the photo breathing room, and
    // matches Linkitylink's card; everything below lays out from `y`.
    let mut y: u32 = 216;
    let mut body = String::new();

    // Avatar
    let cx = WIDTH / 2;
    let cy: u32 = 110;
    let r: u32 = 60;
    if let Some(photo) = &profile.photo {
        body.push_str(&format!(
            r#"<defs><clipPath id="avatarClip"><circle cx="{cx}" cy="{cy}" r="{r}"/></clipPath></defs>
<image href="data:image/jpeg;base64,{photo}" x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="xMidYMid slice" clip-path="url(#avatarClip)"/>
<circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{PALETTE_MINT}" stroke-width="2"/>
"#,
            cx - r,
            cy - r,
            r * 2,
            r * 2,
        ));
    } else {
        body.push_str(&format!(
            r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="{GREEN}"/>
<text x="{cx}" y="{}" font-family="sans-serif" font-size="40" font-weight="bold" fill="{PALETTE_ON_GREEN}" text-anchor="middle">{}</text>
"#,
            cy + 14,
            escape_xml(&get_initials(&name)),
        ));
    }

    // Name
    body.push_str(&format!(
        r#"<text x="{cx}" y="{y}" font-family="sans-serif" font-size="26" font-weight="bold" fill="{PALETTE_MINT}" text-anchor="middle">{}</text>
"#,
        escape_xml(&name),
    ));

    if let Some(title) = profile.title.as_deref().filter(|s| !s.is_empty()) {
        y += 26;
        body.push_str(&format!(
            r#"<text x="{cx}" y="{y}" font-family="sans-serif" font-size="15" fill="{PURPLE}" text-anchor="middle">{}</text>
"#,
            escape_xml(title),
        ));
    }

    if let Some(company) = profile.company.as_deref().filter(|s| !s.is_empty()) {
        y += 22;
        body.push_str(&format!(
            r#"<text x="{cx}" y="{y}" font-family="sans-serif" font-size="13" fill="rgba({PALETTE_INK_RGB},0.6)" text-anchor="middle">{}</text>
"#,
            escape_xml(company),
        ));
    }

    if let Some(bio) = profile.bio.as_deref().filter(|s| !s.is_empty()) {
        y += 34;
        let lines = wrap_text(bio, 42, 3);
        // Quotes wrap the whole bio, not each line. Every line used to get its
        // own pair, so a two-line bio rendered as
        //   "Analytical engines, mostly. Occasionally"
        //   "poetry."
        // which reads as two separate quotations.
        let last = lines.len().saturating_sub(1);
        for (i, line) in lines.iter().enumerate() {
            let open = if i == 0 { "\u{201C}" } else { "" };
            let close = if i == last { "\u{201D}" } else { "" };
            body.push_str(&format!(
                r#"<text x="{cx}" y="{y}" font-family="sans-serif" font-style="italic" font-size="12" fill="rgba({PALETTE_INK_RGB},0.7)" text-anchor="middle">{open}{}{close}</text>
"#,
                escape_xml(line),
            ));
            y += 17;
        }
        y -= 17;
    }

    y += 40;
    let contact_x: u32 = 40;
    let contact_row = |body: &mut String, y: &mut u32, icon: &str, label: String, href: Option<String>| {
        let escaped_label = escape_xml(&label);
        if let Some(href) = href {
            body.push_str(&format!(
                r#"<a href="{}"><text x="{contact_x}" y="{y}" font-family="sans-serif" font-size="14" fill="rgba({PALETTE_INK_RGB},0.85)">{icon}  {escaped_label}</text></a>
"#,
                escape_xml(&href),
            ));
        } else {
            body.push_str(&format!(
                r#"<text x="{contact_x}" y="{y}" font-family="sans-serif" font-size="14" fill="rgba({PALETTE_INK_RGB},0.85)">{icon}  {escaped_label}</text>
"#,
            ));
        }
        *y += 28;
    };

    if let Some(email) = profile.email.as_deref().filter(|s| !s.is_empty()) {
        contact_row(&mut body, &mut y, "@", email.to_string(), Some(format!("mailto:{email}")));
    }
    if let Some(phone) = profile.phone.as_deref().filter(|s| !s.is_empty()) {
        contact_row(&mut body, &mut y, "#", phone.to_string(), Some(format!("tel:{phone}")));
    }
    if let Some(website) = profile.website.as_deref().filter(|s| !s.is_empty()) {
        let href = if website.starts_with("http://") || website.starts_with("https://") {
            website.to_string()
        } else {
            format!("https://{website}")
        };
        contact_row(&mut body, &mut y, "~", website.to_string(), Some(href));
    }
    if let Some(location) = profile.location.as_deref().filter(|s| !s.is_empty()) {
        contact_row(&mut body, &mut y, "*", location.to_string(), None);
    }

    y += 24;
    body.push_str(&format!(
        r#"<text x="{cx}" y="{y}" font-family="sans-serif" font-size="11" fill="rgba({PALETTE_INK_RGB},0.4)" text-anchor="middle">a Freyja offering</text>
"#
    ));

    let height = y + 24;

    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}"><rect x="0" y="0" width="{WIDTH}" height="{height}" fill="{BG}"/>{body}</svg>"#
    )
}

// ── vCard rendering ──────────────────────────────────────────────────────────
//
// Mirrors shared/vcard.js's format exactly (field mapping, no escaping of
// vCard-special characters) so a card looks the same whether it was
// downloaded via the app's native share sheet or savage's "Save Contact"
// button on the public share page. JS can't run in savage's Node process on
// this card's data (it only has whatever's in the published BDO payload), so
// the vCard has to be pre-rendered here at publish time, same as the SVG.

/// Fold a vCard content line per RFC 2426 §2.6: no line may exceed 75
/// characters, and continuation lines start with a single leading space.
fn fold_vcard_line(line: &str) -> String {
    const LIMIT: usize = 75;
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= LIMIT {
        return line.to_string();
    }

    let mut result: String = chars[..LIMIT].iter().collect();
    let mut rest = &chars[LIMIT..];
    while !rest.is_empty() {
        let take = rest.len().min(LIMIT - 1);
        result.push_str("\r\n ");
        result.push_str(&rest[..take].iter().collect::<String>());
        rest = &rest[take..];
    }
    result
}

fn render_vcard(profile: &Profile) -> String {
    let mut lines: Vec<String> = vec!["BEGIN:VCARD".to_string(), "VERSION:3.0".to_string()];

    if let Some(name) = profile.name.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("FN:{name}"));
        let parts: Vec<&str> = name.split(' ').collect();
        if parts.len() >= 2 {
            lines.push(format!("N:{};{};;;", parts[1..].join(" "), parts[0]));
        } else {
            lines.push(format!("N:;{name};;;"));
        }
    }
    if let Some(title) = profile.title.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("TITLE:{title}"));
    }
    if let Some(company) = profile.company.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("ORG:{company}"));
    }
    if let Some(email) = profile.email.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("EMAIL;TYPE=INTERNET:{email}"));
    }
    if let Some(phone) = profile.phone.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("TEL;TYPE=CELL:{phone}"));
    }
    if let Some(website) = profile.website.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL:{website}"));
    }
    if let Some(location) = profile.location.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("ADR;TYPE=WORK:;;{location};;;;"));
    }
    if let Some(bio) = profile.bio.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("NOTE:{bio}"));
    }
    if let Some(handle) = profile.social.instagram.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=Instagram:https://instagram.com/{handle}"));
    }
    if let Some(handle) = profile.social.x.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=X:https://x.com/{handle}"));
    }
    if let Some(handle) = profile.social.tiktok.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=TikTok:https://tiktok.com/@{handle}"));
    }
    if let Some(handle) = profile.social.youtube.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=YouTube:https://youtube.com/@{handle}"));
    }
    if let Some(handle) = profile.social.facebook.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=Facebook:https://facebook.com/{handle}"));
    }
    if let Some(handle) = profile.social.linkedin.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=LinkedIn:https://linkedin.com/in/{handle}"));
    }
    if let Some(handle) = profile.social.github.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=GitHub:https://github.com/{handle}"));
    }
    if let Some(handle) = profile.social.codeberg.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("URL;TYPE=Codeberg:https://codeberg.org/{handle}"));
    }
    if let Some(photo) = profile.photo.as_deref().filter(|s| !s.is_empty()) {
        lines.push(fold_vcard_line(&format!("PHOTO;ENCODING=b;TYPE=JPEG:{photo}")));
    }

    lines.push("END:VCARD".to_string());
    lines.join("\r\n")
}

// ── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
async fn load_cards(app: tauri::AppHandle) -> Result<Vec<Profile>, String> {
    Ok(read_cards(&app)?.cards)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseInfo {
    /// The pinned host, or null if the first card hasn't established one yet.
    pub host: Option<String>,
    pub established_by: Option<String>,
    /// The fifty USPS codes, for the card form's picker — served from Rust so
    /// the list can't drift from the one that maps states to hosts.
    pub states: Vec<String>,
}

/// What base this install is pinned to, and the state list to choose from.
/// Reads only — asking must never pin anything, or merely opening the form
/// would decide the base.
#[tauri::command]
async fn get_base_info(app: tauri::AppHandle) -> Result<BaseInfo, String> {
    let record = read_base(&app);
    Ok(BaseInfo {
        host: record.as_ref().map(|r| r.host.clone()),
        established_by: record.map(|r| r.established_by),
        states: STATE_CODES.iter().map(|s| s.to_string()).collect(),
    })
}

/// Upserts a card by id. Assigns a fresh id if the profile doesn't have one
/// yet (a new card). Rejects new cards once MAX_CARDS is reached.
#[tauri::command]
async fn save_card(app: tauri::AppHandle, mut profile: Profile) -> Result<Profile, String> {
    let mut store = read_cards(&app)?;
    let is_new = profile.id.is_empty() || !store.cards.iter().any(|c| c.id == profile.id);

    if is_new {
        if store.cards.len() >= MAX_CARDS {
            return Err(format!("You can only keep up to {} cards.", MAX_CARDS));
        }
        if profile.id.is_empty() {
            profile.id = new_id();
        }
    }

    store.cards.retain(|c| c.id != profile.id);
    store.cards.push(profile.clone());
    write_cards(&app, &store)?;

    // The first card to name a state pins this install's base. Deliberately
    // after write_cards, so a save that fails doesn't pin anything, and
    // deliberately a no-op once pinned — editing an existing card's state, or
    // adding a second card in another state, must not move a base that cards
    // have already been published against.
    establish_base_from_state(&app, profile.state.as_deref());

    Ok(profile)
}

/// Deletes a card locally AND unpublishes it everywhere it was published.
///
/// Remote first, and local state is kept if the remote fails. Deleting
/// locally on a failed unpublish would discard the uuid and keypair that are
/// the only route back to the published record — leaving the user's name,
/// email, phone and photo public permanently, with no mechanism for anyone to
/// remove them. A retryable error is much better than a permanent orphan.
///
/// The cost is that deletion needs a network connection. That's deliberate.
#[tauri::command]
async fn delete_card(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let store = read_cards(&app)?;
    let card = store
        .cards
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| "Card not found".to_string())?;

    let failed = unpublish_everywhere(&app, &id, BDO_HASH, &card.bdo_uuid_by_env).await;
    if !failed.is_empty() {
        return Err(format!(
            "Couldn't remove the published copy of this card from {}. \
             It's still local, so nothing was lost — check your connection and try again.",
            failed.join(", ")
        ));
    }

    // Only now that the remote is clean. Re-read rather than reusing the store
    // from above, since the await point above means it may be stale.
    let mut store = read_cards(&app)?;
    store.cards.retain(|c| c.id != id);
    write_cards(&app, &store)?;

    // Drop the keypair too — its only purpose was signing for a record that
    // no longer exists, and keeping it would leave a usable credential behind
    // for something the user asked to be deleted.
    let mut keys = read_bdo_keys(&app);
    if keys.remove(&id).is_some() {
        write_bdo_keys(&app, &keys)?;
    }

    Ok(())
}

/// Publishes (or re-publishes, pushing edits) a card to BDO, embedding a
/// rendered SVG so savage can serve it as a live shareable webpage. The link
/// is available the instant this returns — the savage URL is a locally
/// computed signature, not a server-assigned code to wait on.
#[tauri::command]
async fn publish_card(app: tauri::AppHandle, card_id: String) -> Result<Profile, String> {
    let mut store = read_cards(&app)?;
    let index = store
        .cards
        .iter()
        .position(|c| c.id == card_id)
        .ok_or_else(|| "Card not found".to_string())?;

    // Resolve the base once per publish and use it for all three of the
    // client, the env key, and the share URL — they must agree, or a card
    // gets stored under an env key that doesn't match where its uuid lives.
    let base_host = established_base(&app);
    let env_key = base_host_to_env_key(&base_host);

    let sessionless = load_or_create_bdo_sessionless(&app, &card_id)?;
    let client = BDO::new(Some(bdo_url_for(&base_host)), Some(sessionless));

    let svg = render_card_svg(&store.cards[index]);
    let vcard = render_vcard(&store.cards[index]);
    let mut card_json = serde_json::to_value(&store.cards[index]).map_err(|e| e.to_string())?;
    let card_obj = card_json
        .as_object_mut()
        .ok_or_else(|| "card serialized to non-object".to_string())?;
    card_obj.insert("svg".to_string(), serde_json::Value::String(svg));
    card_obj.insert("vcard".to_string(), serde_json::Value::String(vcard));

    // Themes savage's page chrome — the ground behind the card and the "Save
    // Contact" button — to match the card itself. Without this savage falls
    // back to BizBuz's pre-HomeVentory colours for every app it serves, so
    // the card would sit on a near-black page with a bright green button.
    //
    // savage only accepts literal hex here and ignores anything else, since
    // these values land in a style attribute on an otherwise script-free page.
    card_obj.insert(
        "palette".to_string(),
        serde_json::json!({
            "background": PALETTE_BG,
            "accent": PALETTE_GREEN,
            "accentText": PALETTE_ON_GREEN,
        }),
    );

    let existing_uuid = store.cards[index].bdo_uuid_by_env.get(&env_key).cloned();

    let uuid = if let Some(uuid) = existing_uuid {
        client
            .update_bdo(&uuid, BDO_HASH, &card_json, &true)
            .await
            .map_err(|e| e.to_string())?;
        uuid
    } else {
        let user = client
            .create_user(BDO_HASH, &card_json, &true)
            .await
            .map_err(|e| e.to_string())?;
        user.uuid
    };

    let timestamp = unix_now_ms_string();
    let signature = client
        .sessionless
        .sign(format!("{timestamp}{uuid}{BDO_HASH}"))
        .to_hex();
    let share_url = format!(
        "{}user/{uuid}/bdo?timestamp={timestamp}&hash={BDO_HASH}&signature={signature}",
        savage_url_for(&base_host)
    );

    let card = &mut store.cards[index];
    card.bdo_uuid_by_env.insert(env_key, uuid);
    card.share_url = Some(share_url);
    card.published_at = Some(unix_now_ms_string());
    let updated = card.clone();

    write_cards(&app, &store)?;
    Ok(updated)
}

// ── Referral link ────────────────────────────────────────────────────────────
//
// Hosted exactly like a card: an SVG published on its own BDO, wrapped as a
// live webpage by savage. One referral identity per app install (not per
// card — this is about referring new people to the app itself, not tied to
// any specific business card), reusing the same per-key BDO identity
// mechanism as cards (`load_or_create_bdo_sessionless`) under the fixed key
// "referral" rather than a card id.
//
// savage's sanitizer strips <script>/javascript:/data: content from the svg
// (see render_card_svg's doc comment), so an automatic redirect to the App
// Store can't survive being embedded in the svg - instead the svg just has
// a plain "Get BizBuz" button, a real <a href> to the App Store, which is
// exactly the same kind of link the card's own contact rows already use.

const REFERRAL_HASH: &str = "bizbuz-referral";
// TODO: replace with the real App Store listing URL once BizBuz has one.
const APP_STORE_URL: &str = "https://apps.apple.com/app/id0000000000";

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReferralLink {
    uuid: String,
    share_url: String,
}

fn referral_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("referral.json"))
}

// Keyed by the base's env key — same reasoning as Profile.bdo_uuid_by_env, a
// referral link published on one gateway doesn't exist on another.
fn read_referral_links(app: &tauri::AppHandle) -> HashMap<String, ReferralLink> {
    let path = match referral_path(app) {
        Ok(p) => p,
        Err(_) => return HashMap::new(),
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_referral_links(app: &tauri::AppHandle, links: &HashMap<String, ReferralLink>) -> Result<(), String> {
    let path = referral_path(app)?;
    let json = serde_json::to_string_pretty(links).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

/// Renders the referral SVG: a simplified echo of the app icon's
/// stacked-cards mark, a tagline, and a "Get BizBuz" button linking to the
/// App Store.
fn render_referral_svg(app_store_url: &str) -> String {
    const WIDTH: u32 = 400;
    const HEIGHT: u32 = 440;
    const BG: &str = PALETTE_BG;
    const GREEN: &str = PALETTE_GREEN;
    const PURPLE: &str = PALETTE_ACCENT;
    const CARD_BACK: &str = PALETTE_GREEN;

    let cx = WIDTH / 2;
    let button_y: u32 = 300;

    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}">
<defs><linearGradient id="markGradient" x1="0%" y1="0%" x2="100%" y2="100%"><stop offset="0%" stop-color="{PALETTE_MINT}"/><stop offset="100%" stop-color="{PURPLE}"/></linearGradient></defs>
<rect x="0" y="0" width="{WIDTH}" height="{HEIGHT}" fill="{BG}"/>
<rect x="{}" y="58" width="140" height="95" rx="18" fill="{CARD_BACK}"/>
<rect x="{}" y="83" width="140" height="95" rx="18" fill="url(#markGradient)"/>
<circle cx="{}" cy="110" r="11" fill="{BG}"/>
<text x="{cx}" y="216" font-family="sans-serif" font-size="34" font-weight="bold" fill="{PALETTE_MINT}" text-anchor="middle">BizBuz</text>
<text x="{cx}" y="240" font-family="sans-serif" font-size="14" fill="rgba({PALETTE_INK_RGB},0.7)" text-anchor="middle">Digital business cards, made simple.</text>
<text x="{cx}" y="270" font-family="sans-serif" font-size="14" fill="{PURPLE}" text-anchor="middle">You've been invited to try it out.</text>
<a href="{}"><rect x="{}" y="{button_y}" width="240" height="56" rx="16" fill="{GREEN}"/><text x="{cx}" y="{}" font-family="sans-serif" font-size="18" font-weight="bold" fill="{PALETTE_ON_GREEN}" text-anchor="middle">Get BizBuz</text></a>
<text x="{cx}" y="400" font-family="sans-serif" font-size="11" fill="rgba({PALETTE_INK_RGB},0.4)" text-anchor="middle">a Freyja offering</text>
</svg>"#,
        cx - 90,
        cx - 70,
        cx - 55,
        escape_xml(app_store_url),
        cx - 120,
        button_y + 36,
    )
}

/// Returns this install's referral share URL — publishing an svg to its own
/// fresh BDO and computing the permanent savage URL the first time this is
/// called (mirrors `publish_card` exactly), and reusing the result (from
/// local storage) on every call after that, since the referral svg is
/// static and never needs re-publishing.
#[tauri::command]
async fn get_or_create_referral_link(app: tauri::AppHandle) -> Result<String, String> {
    let base_host = established_base(&app);
    let env_key = base_host_to_env_key(&base_host);

    let mut links = read_referral_links(&app);
    if let Some(link) = links.get(&env_key) {
        return Ok(link.share_url.clone());
    }

    let sessionless = load_or_create_bdo_sessionless(&app, "referral")?;
    let client = BDO::new(Some(bdo_url_for(&base_host)), Some(sessionless));

    let svg = render_referral_svg(APP_STORE_URL);
    let card_json = serde_json::json!({ "svg": svg });

    let user = client
        .create_user(REFERRAL_HASH, &card_json, &true)
        .await
        .map_err(|e| e.to_string())?;
    let uuid = user.uuid;

    let timestamp = unix_now_ms_string();
    let signature = client
        .sessionless
        .sign(format!("{timestamp}{uuid}{REFERRAL_HASH}"))
        .to_hex();
    let share_url = format!(
        "{}user/{uuid}/bdo?timestamp={timestamp}&hash={REFERRAL_HASH}&signature={signature}",
        savage_url_for(&base_host)
    );

    let link = ReferralLink { uuid, share_url: share_url.clone() };
    links.insert(env_key, link);
    write_referral_links(&app, &links)?;
    Ok(share_url)
}

// ── Testing: wipe local state ───────────────────────────────────────────────
//
// Removes every file that would carry over "who this device is" from
// BDO's perspective — cards, per-card and referral sessionless keypairs,
// the cached referral link, and the legacy pre-multi-card profile — so
// the next launch is indistinguishable from a fresh install to any
// allyabase service.
//
// Deliberately does NOT touch the App-Group-shared canonical profile
// (`canonical.profile`) or the bizbuz.profile hand-off record — those
// are cross-app state owned jointly with linkitylink/gettit/etc., and
// wiping them here would silently reset those apps too.
#[tauri::command]
async fn reset_all_data(app: tauri::AppHandle) -> Result<(), String> {
    // Unpublish everything first, for the same reason delete_card does: the
    // files removed below are the only record of what was published and the
    // only keys that can authorise its removal. Wiping them while records are
    // still live would orphan every one of them permanently.
    let mut failed: Vec<String> = Vec::new();

    for card in read_cards(&app)?.cards {
        for env_key in unpublish_everywhere(&app, &card.id, BDO_HASH, &card.bdo_uuid_by_env).await {
            failed.push(format!("{} ({})", card.name.clone().unwrap_or_else(|| "card".into()), env_key));
        }
    }

    // The referral card is a separate published record under its own identity
    // ("referral"), and it carries no personal data — but it's still something
    // the user published, so a reset has to take it down too.
    let referral_uuids: HashMap<String, String> = read_referral_links(&app)
        .into_iter()
        .map(|(env_key, link)| (env_key, link.uuid))
        .collect();
    for env_key in unpublish_everywhere(&app, "referral", REFERRAL_HASH, &referral_uuids).await {
        failed.push(format!("referral link ({env_key})"));
    }

    if !failed.is_empty() {
        return Err(format!(
            "Couldn't remove published copies of: {}. Nothing was deleted locally, \
             so you can try again — check your connection first.",
            failed.join(", ")
        ));
    }

    let paths = [
        cards_path(&app)?,
        bdo_keys_path(&app)?,
        referral_path(&app)?,
        legacy_profile_path(&app)?,
        // The pinned base too: "fresh install" has to include which base this
        // install talks to, or a reset would keep testing against whichever
        // state was picked first and never re-exercise the pinning logic.
        base_path(&app)?,
    ];
    for path in paths {
        match fs::remove_file(&path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("Failed to remove {}: {}", path.display(), err)),
        }
    }
    Ok(())
}

// ── App Group sharing ───────────────────────────────────────────────────────

#[tauri::command]
async fn share_card_to_app_group(app: tauri::AppHandle, card_id: String) -> Result<(), String> {
    let store = read_cards(&app)?;
    let card = store
        .cards
        .iter()
        .find(|c| c.id == card_id)
        .ok_or_else(|| "Card not found".to_string())?;
    let json = serde_json::to_string(card).map_err(|e| e.to_string())?;
    tauri_plugin_app_group::write_value_sync(&app, "bizbuz.profile", &json)
}

/// Permissive mirror of Linkitylink's `LinkCard`/`LinkEntry` — every field
/// optional/defaulted so a version skew between the two apps (one ahead of
/// the other on a given phone) never breaks deserialization here.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LinkitylinkEntryMirror {
    label: String,
    url: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LinkitylinkCardMirror {
    name: Option<String>,
    bio: Option<String>,
    photo: Option<String>,
    links: Vec<LinkitylinkEntryMirror>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFromLinkitylinkResult {
    pub name: Option<String>,
    pub bio: Option<String>,
    pub photo: Option<String>,
    pub instagram: Option<String>,
    pub x: Option<String>,
    pub tiktok: Option<String>,
    pub youtube: Option<String>,
    pub facebook: Option<String>,
    pub linkedin: Option<String>,
    pub github: Option<String>,
    pub codeberg: Option<String>,
    pub website: Option<String>,
    pub skipped_count: usize,
}

fn host_and_path(url: &str) -> (String, String) {
    let trimmed = url.trim();
    let lower = trimmed.to_lowercase();
    let after_scheme = if lower.starts_with("https://") {
        &trimmed[8..]
    } else if lower.starts_with("http://") {
        &trimmed[7..]
    } else {
        trimmed
    };
    let mut parts = after_scheme.splitn(2, '/');
    let host = parts
        .next()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_lowercase();
    let path = parts.next().unwrap_or("").trim_end_matches('/').to_string();
    (host, path)
}

/// Scoped-down equivalent of Linkitylink's `detect_platform`, limited to the
/// hosts BizBuz actually has fields for. Returns (field name, handle).
fn social_field_for_url(url: &str) -> Option<(&'static str, String)> {
    let (host, path) = host_and_path(url);
    let first_segment = |p: &str| p.split('/').next().unwrap_or("").trim_start_matches('@').to_string();
    match host.as_str() {
        "instagram.com" => {
            let handle = first_segment(&path);
            if handle.is_empty() { None } else { Some(("instagram", handle)) }
        }
        "x.com" | "twitter.com" => {
            let handle = first_segment(&path);
            if handle.is_empty() { None } else { Some(("x", handle)) }
        }
        "tiktok.com" => {
            let handle = first_segment(&path);
            if handle.is_empty() { None } else { Some(("tiktok", handle)) }
        }
        "youtube.com" | "youtu.be" => {
            let handle = first_segment(&path);
            if handle.is_empty() { None } else { Some(("youtube", handle)) }
        }
        "facebook.com" | "fb.com" => {
            let handle = first_segment(&path);
            if handle.is_empty() { None } else { Some(("facebook", handle)) }
        }
        "linkedin.com" => {
            let rest = path.strip_prefix("in/").unwrap_or(&path);
            let handle = first_segment(rest);
            if handle.is_empty() { None } else { Some(("linkedin", handle)) }
        }
        "github.com" => {
            let handle = first_segment(&path);
            if handle.is_empty() { None } else { Some(("github", handle)) }
        }
        "codeberg.org" => {
            let handle = first_segment(&path);
            if handle.is_empty() { None } else { Some(("codeberg", handle)) }
        }
        _ => None,
    }
}

#[tauri::command]
async fn import_from_linkitylink(app: tauri::AppHandle) -> Result<ImportFromLinkitylinkResult, String> {
    let raw = tauri_plugin_app_group::read_value_sync(&app, "linkitylink.card")?.ok_or_else(|| {
        "Linkitylink hasn't shared anything yet — open Linkitylink and tap \u{201c}Share to App Group\u{201d} first.".to_string()
    })?;
    let card: LinkitylinkCardMirror =
        serde_json::from_str(&raw).map_err(|e| format!("Couldn't read Linkitylink's shared card: {e}"))?;

    let (mut instagram, mut x, mut tiktok, mut youtube, mut facebook) = (None, None, None, None, None);
    let (mut linkedin, mut github, mut codeberg, mut website) = (None, None, None, None);
    let mut skipped = 0usize;

    for link in &card.links {
        if let Some((field, handle)) = social_field_for_url(&link.url) {
            match field {
                "instagram" if instagram.is_none() => instagram = Some(handle),
                "x" if x.is_none() => x = Some(handle),
                "tiktok" if tiktok.is_none() => tiktok = Some(handle),
                "youtube" if youtube.is_none() => youtube = Some(handle),
                "facebook" if facebook.is_none() => facebook = Some(handle),
                "linkedin" if linkedin.is_none() => linkedin = Some(handle),
                "github" if github.is_none() => github = Some(handle),
                "codeberg" if codeberg.is_none() => codeberg = Some(handle),
                _ => skipped += 1,
            }
        } else if website.is_none() {
            website = Some(link.url.clone());
        } else {
            skipped += 1;
        }
    }

    Ok(ImportFromLinkitylinkResult {
        name: card.name,
        bio: card.bio,
        photo: card.photo,
        instagram,
        x,
        tiktok,
        youtube,
        facebook,
        linkedin,
        github,
        codeberg,
        website,
        skipped_count: skipped,
    })
}

// ── Canonical profile ───────────────────────────────────────────────────────
//
// A third, independent record — separate from this app's own CardsStore —
// holding "all of the user's information" in one place, shared verbatim
// across every app in group.freyja.idothis via the App Group plugin. Not
// wired into save_card/publish_card in any way; editing it never touches
// cards.json.

// Higher than MAX_LINKS(10) since this now covers every profile field, not
// just links.
const MAX_PROFILE_FIELDS: usize = 20;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalField {
    pub slug: String,
    pub name: String,
    pub value: String,
}

/// A standard postal address on the shared Canonical Profile. This app has
/// no UI to view or edit it (Gettit does — see its lib.rs) — the field
/// exists here purely so `save_canonical_profile` below can round-trip it
/// without silently erasing whatever Gettit wrote, since every app that
/// touches Canonical Profile overwrites the whole shared record on save.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub street: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub city: String,
    pub state: String,
    pub zip: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalProfile {
    pub photo: Option<String>,
    #[serde(default)]
    pub fields: Vec<CanonicalField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<Address>,
    /// Up to 4 idothis category slugs. Written by idothis; carried forward
    /// unchanged by every other app on save (None means "don't touch",
    /// mirroring the existing address pattern).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idothis_categories: Option<Vec<String>>,
    /// Freelancer's default service ZIP — used by idothis for the discovery
    /// radius filter, cross-shared so other apps could surface it if they
    /// gain a "location" UI later. Carry-forward on save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_zip: Option<String>,
    /// Freelancer's default hourly rate in cents, for idothis listings.
    /// Carry-forward on save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idothis_rate_cents: Option<u64>,
    /// True once the user has connected a Stripe payout destination via
    /// getpayed. Idothis uses this to gate the "Join" action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_connected: Option<bool>,
    pub updated_at: Option<String>,
}

fn slugify(s: &str) -> String {
    let mut slug = String::new();
    let mut last_was_sep = true; // drop leading separators
    for ch in s.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            slug.push('_');
            last_was_sep = true;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    slug
}

#[tauri::command]
async fn load_canonical_profile(app: tauri::AppHandle) -> Result<Option<CanonicalProfile>, String> {
    let raw = tauri_plugin_app_group::read_value_sync(&app, "canonical.profile")?;
    match raw {
        // A decode failure (e.g. leftover data from an earlier schema) is
        // treated as "nothing saved yet" rather than a hard error.
        Some(json) => Ok(serde_json::from_str(&json).ok()),
        None => Ok(None),
    }
}

#[tauri::command]
async fn save_canonical_profile(app: tauri::AppHandle, mut profile: CanonicalProfile) -> Result<CanonicalProfile, String> {
    // This app's own form never sends real values for address / idothis
    // fields / stripe status (no UI for any of them — see the field doc
    // comments above), so always carry forward whatever's already stored
    // rather than overwriting them with the incoming None.
    let existing = load_canonical_profile(app.clone()).await?;
    if let Some(existing) = existing {
        if profile.address.is_none() { profile.address = existing.address; }
        if profile.idothis_categories.is_none() { profile.idothis_categories = existing.idothis_categories; }
        if profile.service_zip.is_none() { profile.service_zip = existing.service_zip; }
        if profile.idothis_rate_cents.is_none() { profile.idothis_rate_cents = existing.idothis_rate_cents; }
        if profile.stripe_connected.is_none() { profile.stripe_connected = existing.stripe_connected; }
    }

    let mut deduped: Vec<CanonicalField> = Vec::new();
    for mut field in profile.fields.into_iter() {
        if field.slug.trim().is_empty() {
            field.slug = slugify(&field.name);
        }
        if field.slug.is_empty() {
            continue;
        }
        deduped.retain(|f| f.slug != field.slug);
        deduped.push(field);
    }
    deduped.truncate(MAX_PROFILE_FIELDS);
    profile.fields = deduped;
    profile.updated_at = Some(unix_now_ms_string());
    let json = serde_json::to_string(&profile).map_err(|e| e.to_string())?;
    tauri_plugin_app_group::write_value_sync(&app, "canonical.profile", &json)?;
    Ok(profile)
}

// ── App entry ────────────────────────────────────────────────────────────────
//
// NOTE: tauri-plugin-quick-actions is temporarily NOT registered — its
// source (Rust + Swift) was lost to an accidental `git clean` and hasn't
// been rebuilt yet. ios-native/BizbuzQuickActionsBridge.m (the native
// swizzling bridge that writes a pending shortcut item to UserDefaults) is
// still here and harmless to compile in, but nothing currently reads that
// value back out to JS. See the frontend for the matching removal of
// syncQuickActions/checkPendingQuickAction.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_share_sheet::init())
        .plugin(tauri_plugin_app_group::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            load_cards,
            get_base_info,
            save_card,
            delete_card,
            publish_card,
            get_or_create_referral_link,
            share_card_to_app_group,
            import_from_linkitylink,
            load_canonical_profile,
            save_canonical_profile,
            reset_all_data
        ])
        .run(tauri::generate_context!())
        .expect("error while running bizbuz");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a fully-populated card and writes it out, so the published SVG
    /// can actually be looked at rather than reasoned about. Colour changes in
    /// particular are not reviewable by reading hex constants — the text fills
    /// were white-on-dark, and flipping the ground to glacier white without
    /// flipping them would have shipped an invisible card.
    ///
    ///   cargo test render_sample_card -- --nocapture
    ///   rsvg-convert -w 400 /tmp/bizbuz-card-sample.svg -o /tmp/card.png
    #[test]
    fn render_sample_card() {
        let mut social = Social::default();
        social.instagram = Some("ada".into());
        social.github = Some("ada".into());

        let profile = Profile {
            id: "sample".into(),
            name: Some("Ada Lovelace".into()),
            title: Some("Software Enchantress".into()),
            company: Some("Freyja - Love and Magic".into()),
            email: Some("ada@example.com".into()),
            phone: Some("+1 (555) 123-4567".into()),
            website: Some("example.com".into()),
            location: Some("Portland, OR".into()),
            state: Some("OR".into()),
            bio: Some("Analytical engines, mostly. Occasionally poetry.".into()),
            social,
            photo: None,
            ..Default::default()
        };

        let svg = render_card_svg(&profile);
        std::fs::write("/tmp/bizbuz-card-sample.svg", &svg).unwrap();

        // Guard the mistake that prompted this test: no white-on-white text.
        assert!(!svg.contains("rgba(255,255,255"), "white text on a light card");
        assert!(svg.contains(PALETTE_BG), "card ground should use the palette");
        println!("wrote /tmp/bizbuz-card-sample.svg ({} bytes)", svg.len());
    }

    #[test]
    fn render_sample_referral() {
        let svg = render_referral_svg(APP_STORE_URL);
        std::fs::write("/tmp/bizbuz-referral-sample.svg", &svg).unwrap();
        assert!(!svg.contains("rgba(255,255,255"), "white text on a light card");
        println!("wrote /tmp/bizbuz-referral-sample.svg ({} bytes)", svg.len());
    }
}
