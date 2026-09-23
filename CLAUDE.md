# BizBuz - Development Documentation

## Overview

BizBuz is a Planet Nine native digital-business-card app: a user builds up to 4 cards (work, personal, side hustle, etc.), each rendered as a shareable SVG card and published to BDO so it has a permanent public link (served live by savage). Cards can also be exported as a standard vCard via the OS share sheet. Separately, BizBuz hosts one screen of the cross-app **Canonical Profile** — a single shared identity record synced to sibling apps via an iOS App Group — but that record is independent of the cards themselves; the two never touch.

**Location**: `/bizbuz/app/`
**Identifier**: `com.freyja.bizbuz`
**Stack**: Tauri 2 (Rust core in `src-tauri/`, vanilla JS/HTML/CSS frontend in `src/`, no framework/bundler), targeting iOS.
**Status**: Actively developed / TestFlight builds (build 21 as of this writing).

## Architecture

### Frontend (`app/src/`)

Three files plus one synced module:
- `index.html` — four views as sibling `<section>`s toggled via a `hidden` attribute: `cards-view` (grid of up to 4 cards), `edit-view` (the card form), `card-view` (a single card's read view + share actions), `profile-view` (the separate Canonical Profile editor).
- `main.js` — all view logic (794 lines), calling into the Rust backend via `core.invoke(...)`.
- `style.css` — dark/green/purple visual language shared across the Planet Nine app family.
- `src/lib/vcard.js` — **not authored here**. `scripts/sync-shared.cjs` copies it in from `/bizbuz/shared/vcard.js` on every `predev`/`build` so the frontend can `import` it as a local module without a bundler resolving paths outside `src/`. It's a pure vCard 3.0 formatter shared with the legacy `server.js` (see Known Limitations).

### Backend (`app/src-tauri/src/lib.rs`)

Tauri commands, invoked from `main.js`:

| Command | Purpose |
|---|---|
| `get_categories` | Returns the static business-category taxonomy (slug/label pairs shared with idothis) |
| `load_cards` / `save_card` / `delete_card` | Local multi-card store (`cards.json`), capped at `MAX_CARDS = 4` |
| `publish_card` | Renders the card as SVG + vCard, publishes/republishes it to BDO, computes the permanent savage share link |
| `get_or_create_referral_link` | Publishes (once per install) a static "invite a friend" SVG card to its own BDO identity, for sharing the app itself |
| `share_card_to_app_group` | Writes the active card's JSON into the App Group under key `bizbuz.profile`, for Linkitylink to import |
| `import_from_linkitylink` | Reads `linkitylink.card` from the App Group and maps its links into BizBuz's name/bio/photo/website/social fields |
| `load_canonical_profile` / `save_canonical_profile` | The shared cross-app Canonical Profile (see below) |

No server of its own for the live app — BizBuz is a thin client over two allyabase services. Which base is per-install, not compiled in: the state on the user's FIRST card pins them to `https://<state>.8as.world/`, with `prod.8as.world` as the fallback and `dev.8as.world` retained for installs that published before per-state bases existed. See the "Which base this install talks to" block in `lib.rs`.

| Service | Const | Role |
|---|---|---|
| **BDO** | `GATEWAY_BDO_URL` | Public storage for each published card and the referral card — one sessionless keypair per card (BDO's public slot is keyed by pubKey, so one shared identity would let every card overwrite the others; verified against allyabase's `db.js`) |
| **savage** | `SAVAGE_URL` | Renders whatever `svg` field is present on a published BDO record as a live webpage, at a locally pre-signed, non-expiring URL — no round trip needed to get a shareable link |

Each card's `bdoUuidByEnv` is keyed by the base's hostname with dots swapped for dashes (`ca-8as-world`), so a card published against one base is never confused with another — BDO mints its own uuid per base and a uuid from one 404s against another. The pin in `base.json` is write-once for the same reason.

### Card lifecycle

1. **Create/Edit** (`save_card`): upserts a card into `cards.json` by id, assigning a fresh id for new cards. Rejects a new card once 4 already exist.
2. **Publish** (`publish_card`): generates/reuses a per-card BDO keypair (`bdo_keys.json`), renders the card as both an SVG (`render_card_svg`, avatar/name/title/company/bio/contact rows in the app's dark/green/purple palette) and a vCard (`render_vcard`), and either creates or updates the BDO record. Every save triggers a background republish automatically (`backgroundPublishCard`) — there's no explicit "republish" button, saving again is the retry on failure.
3. **Share**: the "Share" button hands a generated vCard (via `src/lib/vcard.js`) to the OS share sheet; the card view also shows the permanent savage `shareUrl` with a Copy button once published.
4. **Cross-app exchange**: "Import from Linkitylink" pulls `linkitylink.card` from the App Group into the currently-open form (only filling fields still empty); "Share to App Group" pushes the active card out for Linkitylink to pick up. Saving a card whose category is food-related (caterer, restauranteur, chef, food cart, baker) offers a one-time prompt to also list it on letemcook via a `letemcook://add-location` deep link.
5. **Referral**: a separate "Share BizBuz" button on the cards list publishes (once per install, then reuses) a static invite-card SVG to its own BDO identity and hands the resulting savage URL to the share sheet — used to invite others to the app itself, not tied to any specific business card.

### Canonical Profile (shared across apps)

A third, independent record — separate from `cards.json` entirely — synced via the **`group.freyja.idothis`** iOS App Group, the same group Linkitylink, Gettit, and Letemcook read/write. Read/written through `tauri-plugin-app-group`'s `read_value_sync`/`write_value_sync` under the key `canonical.profile`. Its shape is a photo plus an ordered, user-editable list of `{slug, name, value}` fields (capped at `MAX_PROFILE_FIELDS = 20`), not the fixed schema cards use. "Fill from My Cards" pulls first-non-empty values out of the locally-loaded `cards` array without a round trip. The Rust struct also carries an `address` field BizBuz has no UI for (Gettit does) — `save_canonical_profile` always carries forward whatever address is already stored rather than clobbering it with `None`, since every app that touches this record overwrites the whole thing on save. This logic is intentionally copy-pasted byte-for-byte across the sibling apps rather than shared as a library.

## Build & Deploy

`app/package.json` scripts (each `dev`/`build` runs `predev`/`sync-shared.cjs` first to refresh `src/lib/vcard.js`):
- `npm run dev` — `tauri dev`
- `npm run build` — desktop `tauri build`
- `npm run build:ios` — `scripts/build-ios.cjs`, the real distribution path (produces a signed IPA)
- `npm run ios:dev` — `tauri ios dev`
- `npm run android:dev` / `android:build` — present but unverified as an active target

### `scripts/build-ios.cjs`

1. Syncs `shared/vcard.js` into `src/lib/`.
2. Bumps `.build-number` (App Store Connect rejects re-uploading the same `CFBundleVersion`).
3. Wipes and regenerates `src-tauri/gen/apple/` via `tauri ios init` — a clean slate every time.
4. Patches several things back in that don't survive that regeneration: the `ios-native/` source path (Quick Actions swizzling bridge + `PrivacyInfo.xcprivacy`), `TARGETED_DEVICE_FAMILY` restricted to iPhone-only (the UI is a fixed phone-sized window with no iPad layout or screenshots), `ITSAppUsesNonExemptEncryption: false` (skips the App Store Connect encryption questionnaire), the real app icon (`tauri ios init` reverts to Tauri's stock icon), flattened alpha channels on all icon sizes (App Store rejects an alpha channel on the 1024×1024 marketing icon), and the App Group entitlement (`group.freyja.idothis`).
5. `tauri ios build --export-method app-store-connect --build-number <n>`, with a manual `xcodebuild -exportArchive` fallback for a known Xcode 26 export-plist quirk. Before attempting that fallback, the script checks the keychain for an actual Apple/iOS Distribution signing identity and aborts loudly if only a Development identity is present — builds 5–7 silently re-signed with the wrong identity type via this same fallback path and "succeeded" locally while producing IPAs that Apple's server-side validation rejected on upload.
6. Copies the resulting IPA to `builds/v{version}/{ProductName}-{buildNumber}.ipa`.

Upload to App Store Connect is a deliberately separate, manual step (Transporter.app or `xcrun altool`) — the script never uploads anything itself. There's also a top-level `~/Work/planet-nine/builds/` folder holding one "latest" IPA per sibling app as a manually-maintained convenience copy — not written by this script.

## Version trains

App Store Connect closes a version train once a build has been submitted
under it, and answers any further upload with:

    Validation failed (409) Invalid Pre-Release Train. The train version
    'X.Y.Z' is closed for new build submissions

So every submission round needs a **new** version, and `CFBundleVersion` only
has to be unique *within* a train, which means the build number restarts at 1
rather than continuing. Bumping means three files plus the counter, and all
four have to agree or the build lands in the wrong folder or gets rejected:

- `app/src-tauri/tauri.conf.json` (drives `CFBundleShortVersionString` and the
  `builds/vX.Y.Z/` folder `build-ios.cjs` writes to)
- `app/package.json`
- `app/src-tauri/Cargo.toml`
- `app/.build-number` set to `0`, since the script writes `previous + 1`

Trains used so far: 0.1.0 (released as 1.0 on the store), 0.2.0, 0.3.0. **The
App Store version string and the version here do not correspond**: the listing
showed 1.0 while this repo was on 0.1.0, which is expected and not worth
reconciling.

## Known Limitations (documented, not oversights)

- **Quick Actions plugin not registered**: `tauri-plugin-quick-actions`'s source (Rust + Swift) was lost to an accidental `git clean` and hasn't been rebuilt. The native `ios-native/BizbuzQuickActionsBridge.m` swizzling bridge still compiles in and is harmless, but nothing currently reads its pending-shortcut value back out to JS.
- **Referral cards are published once and cached, so what they assert has to be correctable**: the record lives at a permanent savage URL that may already be shared widely, so `get_or_create_referral_link` updates it IN PLACE rather than minting a new link whenever something it asserts has changed — the payout destination (after connecting Stripe in getpayed) or the App Store URL. `ReferralLink` remembers both so a change can be detected; `app_store_url: None` means the record predates that tracking and is rewritten once. BizBuz's real listing id is **6808065888** (set September 2026); note the public page 404s until the app is actually released.
- **Referral card has no auto-redirect**: savage strips `<script>`/`javascript:`/`data:` content from any published SVG, so the invite card can't auto-navigate to the App Store — it's a plain `<a href>` "Get BizBuz" button instead, the same mechanism the card's own contact rows use.
- **vCard has no special-character escaping**: `render_vcard` in Rust deliberately mirrors `shared/vcard.js`'s format field-for-field (including its lack of escaping) so a card looks identical whether downloaded from the app's share sheet or savage's page.
- **Legacy Express implementation still in the repo**: `/bizbuz/server.js`, `/bizbuz/public/`, and the root `package.json` are the original pre-Tauri Express server (Prof-integration based, per its own `README.md`). It is not part of the Tauri build or app runtime today — the only thing still consumed from that era is `shared/vcard.js`, pulled in by `scripts/sync-shared.cjs`.

## Related Documentation

- Sibling apps sharing the same Tauri scaffolding, App Group, and build-ios.cjs pattern: Gelder, Linkitylink, Gettit, Letemcook, idothis (see their own `CLAUDE.md`)

## Last Updated
September 2, 2026 — Full rewrite. The previous version of this file described an obsolete plain-Express-server architecture ("Initial Implementation (November 2025)") that no longer reflects the codebase; BizBuz is now a Tauri 2 native iOS app, documented here against the actual current source.
