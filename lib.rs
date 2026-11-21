//! Retium NFT Mint & Market. Draft v0.1.0 buat direview.
//!
//! Status: ditulis ngikutin pola kontrak Retium yang sudah ada (call API,
//! storage, event, payout RTM). Belum dicek ke standar RCP NFT resmi dan
//! belum dibuild pakai SDK asli. Semua yang ditandai TODO(RCP) harus
//! disamakan dulu dengan standar yang sudah terverifikasi.
//!
//! Desain:
//!
//! * Satu kontrak bersama nyimpen semua token. ID token berurutan.
//! * Gambar nggak disimpan on-chain. Token cuma nyimpen nama, URI gambar,
//!   dan hash SHA-256 gambar biar integritasnya bisa dicek siapa aja.
//! * Listing cuma penanda harga. NFT nggak pernah keluar dari akun pemilik,
//!   jadi nggak ada escrow.
//! * `buy` itu satu call atomik: ganti owner, hapus listing, bayar seller
//!   langsung, lalu balikin kelebihan bayar. Kalau ada payout yang gagal,
//!   seluruh call dibatalkan.
//! * `buy` pakai `max_price`, jadi kalau seller naikin harga tepat sebelum
//!   buyer call, buyer nggak bisa kelebihan bayar.
//! * Nggak ada admin, init, atau fee. Kontrak immutable setelah deploy.
//! * Index owner dan listing pakai linked list, jadi paging nggak scan semua
//!   key. Cursor sifatnya best-effort kalau list berubah di antara dua
//!   request page.

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use retium_contract::{call, entrypoint, env, error, events, fail, ok, storage, Call, Reply};

entrypoint!(handle);

// TODO(RCP): ganti rcp_type, traits, capabilities, dan weight dengan nilai
// yang diminta standar RCP NFT resmi.
retium_contract::rcp_metadata!(
    r#"{"rcp_type":"nft","traits":["nft","marketplace"],"capabilities":["execute"],"lockable_fields":[],"evolves_from":"","weight":5,"version":"v0.1.0"}"#
);

const VERSION: &str = "v0.1.0";
/// Batas maksimal panjang nama token, dalam byte.
const MAX_NAME_BYTES: usize = 48;
/// Batas maksimal panjang URI gambar, dalam byte.
const MAX_URI_BYTES: usize = 256;
/// Panjang hash SHA-256 dalam hex (64 karakter).
const HASH_LEN: usize = 64;
/// Kalau limit nggak dikasih, paging pakai ukuran ini.
const DEFAULT_PAGE_SIZE: usize = 25;
/// Batas atas limit paging, biar query nggak berat.
const MAX_PAGE_SIZE: usize = 50;
/// Biar list yang korup nggak menghabiskan fuel satu query.
const MAX_LIST_WALK: usize = 500;
/// Namespace linked list buat daftar token yang sedang dijual.
const MARKET_LIST: &str = "mkt";

pub fn handle(call: Call) -> Reply {
    // Cuma `buy` yang nerima RTM. Action lain, termasuk view, nolak.
    if call.action() != "buy" && call.value() > 0 {
        return error("this action is not payable");
    }

    match call.action() {
        "mint" => mint(&call),
        "transfer" => transfer(&call),
        "list" => list_token(&call),
        "unlist" => unlist(&call),
        "buy" => buy(&call),
        "get_token" => get_token(&call),
        "tokens_by_owner" => tokens_by_owner(&call),
        "listed_tokens" => listed_tokens(&call),
        "total_supply" => total_supply(&call),
        "get_config" => get_config(&call),
        other => error(&format!("unknown action '{other}'")),
    }
}

// --- HELPER KECIL -----------------------------------------------------------

fn now_ms() -> u64 {
    env::block_time_ms().max(0) as u64
}

/// Abort setelah state sudah ditulis. Kalau gagal setelah write pertama,
/// seluruh call harus dibatalkan, jadi ini trap bukan return error.
fn must(result: Result<(), String>) {
    if let Err(reason) = result {
        fail(&reason);
    }
}

fn must_set(key: &str, value: &str) {
    must(storage::set(key, value));
}

/// Kirim RTM dan pastikan ledger nerima. `send_rtm` bisa balikin `Ok` padahal
/// kontrak RTM-nya jawab error, jadi cek reply-nya juga.
fn must_pay(to: &str, amount: u128, what: &str) {
    if amount == 0 {
        return;
    }
    match call::send_rtm(to, amount) {
        Ok(reply) if !call::is_error_reply(&reply) => {}
        Ok(_) => {
            fail(&format!("{what}: RTM transfer was rejected"));
        }
        Err(reason) => {
            fail(&format!("{what}: {reason}"));
        }
    }
}

fn page_limit(raw: Option<&str>) -> usize {
    match raw.and_then(|value| value.parse::<usize>().ok()) {
        Some(value) if value > 0 => value.min(MAX_PAGE_SIZE),
        _ => DEFAULT_PAGE_SIZE,
    }
}

fn json_array(items: &[String]) -> String {
    let mut out = String::from("[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&events::json_string(item));
    }
    out.push(']');
    out
}

// --- VALIDASI ---------------------------------------------------------------

/// Cuma cek bentuk. Kontrak nggak bisa hitung checksum address, jadi frontend
/// wajib verifikasi checksum sebelum minta wallet tanda tangan.
fn is_valid_address(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 72 || &bytes[..4] != b"rete" {
        return false;
    }
    bytes[4..]
        .iter()
        .all(|b| (*b >= b'0' && *b <= b'9') || (*b >= b'a' && *b <= b'f'))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == HASH_LEN
        && value
            .bytes()
            .all(|b| (b >= b'0' && b <= b'9') || (b >= b'a' && b <= b'f'))
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name cannot be empty".to_string());
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(format!("name must be at most {MAX_NAME_BYTES} bytes"));
    }
    if name.trim() != name {
        return Err("name cannot start or end with whitespace".to_string());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("name cannot contain control characters".to_string());
    }
    Ok(())
}

fn validate_uri(uri: &str) -> Result<(), String> {
    if uri.len() > MAX_URI_BYTES {
        return Err(format!("image URI must be at most {MAX_URI_BYTES} bytes"));
    }
    let rest = ["https://", "ipfs://", "ar://"]
        .iter()
        .find_map(|scheme| uri.strip_prefix(scheme));
    match rest {
        Some(rest) if !rest.is_empty() => {}
        _ => return Err("image URI must start with https://, ipfs:// or ar://".to_string()),
    }
    // Cuma ASCII printable tanpa spasi.
    if !uri.bytes().all(|b| b > 0x20 && b < 0x7f) {
        return Err("image URI can only contain printable ASCII without spaces".to_string());
    }
    Ok(())
}

/// Parse token id dan balikin bentuk kanoniknya, jadi "01" dan "1" jadi satu
/// key yang sama.
fn canonical_id(raw: &str) -> Result<String, String> {
    match raw.parse::<u64>() {
        Ok(value) if value > 0 => Ok(value.to_string()),
        _ => Err("invalid token id".to_string()),
    }
}

// --- KEY STORAGE ------------------------------------------------------------

fn owner_key(id: &str) -> String {
    format!("owner:{id}")
}
fn creator_key(id: &str) -> String {
    format!("creator:{id}")
}
fn name_key(id: &str) -> String {
    format!("name:{id}")
}
fn uri_key(id: &str) -> String {
    format!("uri:{id}")
}
fn hash_key(id: &str) -> String {
    format!("hash:{id}")
}
fn created_key(id: &str) -> String {
    format!("created:{id}")
}
/// Teks kosong artinya "nggak sedang dijual".
fn price_key(id: &str) -> String {
    format!("price:{id}")
}
fn owner_list(owner: &str) -> String {
    format!("own:{owner}")
}

fn owner_of(id: &str) -> Result<String, String> {
    let owner = storage::get_or(&owner_key(id), "")?;
    if owner.is_empty() {
        return Err("token not found".to_string());
    }
    Ok(owner)
}

// --- INDEX LINKED LIST ------------------------------------------------------
//
// Satu list per namespace (`mkt` buat listing, `own:<address>` per owner).
// Node yang sudah dihapus tetap nyimpen pointer `next`, jadi cursor yang
// nunjuk ke node yang sudah dihapus masih bisa maju. Reader bakal skip node
// yang bukan member.

fn node_key(ns: &str, part: &str, id: &str) -> String {
    format!("{ns}:{part}:{id}")
}
fn end_key(ns: &str, part: &str) -> String {
    format!("{ns}:{part}")
}

fn list_has(ns: &str, id: &str) -> Result<bool, String> {
    Ok(!storage::get_or(&node_key(ns, "in", id), "")?.is_empty())
}

fn list_push(ns: &str, id: &str) -> Result<(), String> {
    if list_has(ns, id)? {
        return Ok(());
    }
    let tail = storage::get_or(&end_key(ns, "tail"), "")?;
    if tail.is_empty() {
        storage::set(&end_key(ns, "head"), id)?;
    } else {
        storage::set(&node_key(ns, "next", &tail), id)?;
    }
    storage::set(&node_key(ns, "prev", id), &tail)?;
    storage::set(&node_key(ns, "next", id), "")?;
    storage::set(&end_key(ns, "tail"), id)?;
    storage::set(&node_key(ns, "in", id), "1")
}

fn list_remove(ns: &str, id: &str) -> Result<(), String> {
    if !list_has(ns, id)? {
        return Ok(());
    }
    let prev = storage::get_or(&node_key(ns, "prev", id), "")?;
    let next = storage::get_or(&node_key(ns, "next", id), "")?;

    if prev.is_empty() {
        storage::set(&end_key(ns, "head"), &next)?;
    } else {
        storage::set(&node_key(ns, "next", &prev), &next)?;
    }
    if next.is_empty() {
        storage::set(&end_key(ns, "tail"), &prev)?;
    } else {
        storage::set(&node_key(ns, "prev", &next), &prev)?;
    }
    storage::set(&node_key(ns, "in", id), "")?;
    storage::set(&node_key(ns, "prev", id), "")
}

fn list_page(ns: &str, after: &str, limit: usize) -> Result<(Vec<String>, String), String> {
    let mut cursor = if after.is_empty() {
        storage::get_or(&end_key(ns, "head"), "")?
    } else {
        storage::get_or(&node_key(ns, "next", after), "")?
    };

    let mut page = Vec::new();
    let mut last = String::new();
    let mut steps = 0usize;
    while !cursor.is_empty() && page.len() < limit && steps < MAX_LIST_WALK {
        let next = storage::get_or(&node_key(ns, "next", &cursor), "")?;
        if list_has(ns, &cursor)? {
            page.push(cursor.clone());
        }
        last = cursor;
        cursor = next;
        steps += 1;
    }
    let next_cursor = if cursor.is_empty() { String::new() } else { last };
    Ok((page, next_cursor))
}

/// Pindahin token ke owner baru. Listing ikut dibersihin. Cuma panggil
/// setelah validasi selesai.
fn move_token(id: &str, from: &str, to: &str) {
    must_set(&owner_key(id), to);
    must_set(&price_key(id), "");
    must(list_remove(MARKET_LIST, id));
    must(list_remove(&owner_list(from), id));
    must(list_push(&owner_list(to), id));
}

// --- ACTION TULIS -----------------------------------------------------------

/// mint(name, image_uri, image_hash)
pub fn mint(call: &Call) -> Reply {
    let caller = call.caller();
    let name = match call.arg(0) {
        Ok(value) => value,
        Err(_) => return error("missing name"),
    };
    let uri = match call.arg(1) {
        Ok(value) => value,
        Err(_) => return error("missing image URI"),
    };
    let hash = match call.arg(2) {
        Ok(value) => value,
        Err(_) => return error("missing image hash"),
    };

    if let Err(reason) = validate_name(name) {
        return error(&reason);
    }
    if let Err(reason) = validate_uri(uri) {
        return error(&reason);
    }
    if !is_sha256_hex(hash) {
        return error("image hash must be 64 lowercase hex characters (SHA-256)");
    }

    let supply_text = match storage::get_or("supply", "0") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let supply: u64 = match supply_text.parse() {
        Ok(value) => value,
        Err(_) => return error("supply state is corrupt"),
    };
    let id = match supply.checked_add(1) {
        Some(value) => value.to_string(),
        None => return error("token id overflow"),
    };

    // Mulai dari sini, semua kegagalan harus batalin seluruh call.
    must_set("supply", &id);
    must_set(&owner_key(&id), caller);
    must_set(&creator_key(&id), caller);
    must_set(&name_key(&id), name);
    must_set(&uri_key(&id), uri);
    must_set(&hash_key(&id), hash);
    must_set(&created_key(&id), &now_ms().to_string());
    must(list_push(&owner_list(caller), &id));

    must(events::emit_fields(
        "token_minted",
        &[
            ("id", &id),
            ("owner", caller),
            ("name", name),
            ("uri", uri),
            ("hash", hash),
        ],
    ));
    ok(&id)
}

/// transfer(token_id, to)
pub fn transfer(call: &Call) -> Reply {
    let caller = call.caller();
    let id = match call.arg(0).map_err(|e| e).and_then(canonical_id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let to = match call.arg(1) {
        Ok(value) => value,
        Err(_) => return error("missing recipient"),
    };
    if !is_valid_address(to) {
        return error("recipient must be a valid Retium address");
    }
    let owner = match owner_of(&id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    if owner != caller {
        return error("only the owner can transfer this token");
    }
    if to == caller {
        return error("recipient is already the owner");
    }

    move_token(&id, &owner, to);
    must(events::emit_fields(
        "token_transferred",
        &[("id", &id), ("from", caller), ("to", to)],
    ));
    ok("transferred")
}

/// list(token_id, price). Price itu integer dalam satuan atomic RTM. Kalau
/// dipanggil lagi pada token yang sudah listing, harganya diganti.
pub fn list_token(call: &Call) -> Reply {
    let caller = call.caller();
    let id = match call.arg(0).map_err(|e| e).and_then(canonical_id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let price: u128 = match call.arg(1).ok().and_then(|value| value.parse().ok()) {
        Some(value) if value > 0 => value,
        _ => return error("price must be a positive integer in atomic RTM"),
    };
    let owner = match owner_of(&id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    if owner != caller {
        return error("only the owner can list this token");
    }

    must_set(&price_key(&id), &price.to_string());
    must(list_push(MARKET_LIST, &id));
    must(events::emit_fields(
        "token_listed",
        &[("id", &id), ("price", &price.to_string()), ("by", caller)],
    ));
    ok("listed")
}

/// unlist(token_id)
pub fn unlist(call: &Call) -> Reply {
    let caller = call.caller();
    let id = match call.arg(0).map_err(|e| e).and_then(canonical_id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let owner = match owner_of(&id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    if owner != caller {
        return error("only the owner can unlist this token");
    }
    let current_price = match storage::get_or(&price_key(&id), "") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    if current_price.is_empty() {
        return error("token is not listed");
    }

    must_set(&price_key(&id), "");
    must(list_remove(MARKET_LIST, &id));
    must(events::emit_fields(
        "token_unlisted",
        &[("id", &id), ("by", caller)],
    ));
    ok("unlisted")
}

/// buy(token_id, max_price). RTM-nya nempel di call.
pub fn buy(call: &Call) -> Reply {
    let buyer = call.caller();
    let paid = call.value();
    let id = match call.arg(0).map_err(|e| e).and_then(canonical_id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let max_price: u128 = match call.arg(1).ok().and_then(|value| value.parse().ok()) {
        Some(value) => value,
        None => return error("max_price must be an integer in atomic RTM"),
    };

    let owner = match owner_of(&id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let price_text = match storage::get_or(&price_key(&id), "") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    if price_text.is_empty() {
        return error("token is not listed for sale");
    }
    let price: u128 = match price_text.parse() {
        Ok(value) if value > 0 => value,
        _ => return error("listing price is invalid"),
    };
    if owner == buyer {
        return error("you cannot buy your own token");
    }
    if price > max_price {
        return error("price is above your maximum");
    }
    if paid < price {
        return error(&format!(
            "insufficient payment: required {} atomic units, sent {}",
            price, paid
        ));
    }

    // Mulai dari sini, semua kegagalan harus batalin seluruh call.
    move_token(&id, &owner, buyer);
    must_pay(&owner, price, "could not pay the seller");
    must_pay(buyer, paid - price, "could not refund the excess");

    must(events::emit_fields(
        "token_sold",
        &[
            ("id", &id),
            ("buyer", buyer),
            ("seller", &owner),
            ("price", &price.to_string()),
        ],
    ));
    ok(&format!("token {} bought for {} atomic units", id, price))
}

// --- ACTION BACA ------------------------------------------------------------

/// get_token(token_id)
pub fn get_token(call: &Call) -> Reply {
    let id = match call.arg(0).map_err(|e| e).and_then(canonical_id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let owner = match owner_of(&id) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let creator = match storage::get_or(&creator_key(&id), "") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let name = match storage::get_or(&name_key(&id), "") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let uri = match storage::get_or(&uri_key(&id), "") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let hash = match storage::get_or(&hash_key(&id), "") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let created = match storage::get_or(&created_key(&id), "0") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let price = match storage::get_or(&price_key(&id), "") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    let is_listed = !price.is_empty();
    let price_display = if is_listed { price } else { "0".to_string() };

    // Angka dikirim sebagai string biar client JavaScript nggak kehilangan
    // presisi.
    ok(&format!(
        r#"{{"id":{},"owner":{},"creator":{},"name":{},"image_uri":{},"image_hash":{},"created_at":{},"price":{},"is_listed":{}}}"#,
        events::json_string(&id),
        events::json_string(&owner),
        events::json_string(&creator),
        events::json_string(&name),
        events::json_string(&uri),
        events::json_string(&hash),
        events::json_string(&created),
        events::json_string(&price_display),
        is_listed
    ))
}

fn paged_reply(ns: &str, after: &str, limit_raw: Option<&str>) -> Reply {
    let after = if after.is_empty() {
        String::new()
    } else {
        match canonical_id(after) {
            Ok(value) => value,
            Err(reason) => return error(&reason),
        }
    };
    let (page, next) = match list_page(ns, &after, page_limit(limit_raw)) {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    ok(&format!(
        r#"{{"tokens":{},"next":{}}}"#,
        json_array(&page),
        events::json_string(&next)
    ))
}

/// tokens_by_owner(owner, after_id, limit)
pub fn tokens_by_owner(call: &Call) -> Reply {
    let owner = match call.arg(0) {
        Ok(value) if is_valid_address(value) => value,
        _ => return error("owner must be a valid Retium address"),
    };
    paged_reply(
        &owner_list(owner),
        call.arg(1).unwrap_or(""),
        call.arg(2).ok(),
    )
}

/// listed_tokens(after_id, limit)
pub fn listed_tokens(call: &Call) -> Reply {
    paged_reply(MARKET_LIST, call.arg(0).unwrap_or(""), call.arg(1).ok())
}

pub fn total_supply(_call: &Call) -> Reply {
    let supply = match storage::get_or("supply", "0") {
        Ok(value) => value,
        Err(reason) => return error(&reason),
    };
    ok(&supply)
}

pub fn get_config(_call: &Call) -> Reply {
    ok(&format!(
        r#"{{"version":{},"max_name_bytes":"{}","max_uri_bytes":"{}","hash_length":"{}","uri_schemes":["https","ipfs","ar"]}}"#,
        events::json_string(VERSION),
        MAX_NAME_BYTES,
        MAX_URI_BYTES,
        HASH_LEN
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use retium_contract::test_host;

    const NOW: i64 = 1_800_000_000_000;
    const RTM: u128 = 1_000_000_000_000_000_000;

    fn addr(c: char) -> String {
        format!("rete{}", c.to_string().repeat(68))
    }
    fn hash() -> String {
        "a".repeat(64)
    }

    fn run(action: &str, caller: &str, args: &[&str], value: u128) -> Reply {
        let raw = serde_json::json!({
            "action": action, "caller": caller, "args": args, "value": value.to_string()
        })
        .to_string();
        handle(Call::from_json(&raw).expect("test call must parse"))
    }

    /// Niru perilaku chain: call yang trap atau balikin error reply
    /// membatalkan semua write.
    fn run_with_rollback(action: &str, caller: &str, args: &[&str], value: u128) -> Reply {
        let snapshot = test_host::snapshot();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(action, caller, args, value)
        })) {
            Ok(reply) => {
                if reply.is_err() {
                    test_host::restore(snapshot);
                }
                reply
            }
            Err(_) => {
                test_host::restore(snapshot);
                Err("contract trapped".to_string())
            }
        }
    }

    fn setup() {
        test_host::reset();
        test_host::set_time_ms(NOW);
        test_host::set_call_reply(call::RTM, "transfer", r#"{"ok":true}"#);
    }

    fn mint_as(who: &str) -> String {
        run_with_rollback("mint", who, &["Cat", "ipfs://bafy123", &hash()], 0).unwrap()
    }

    fn page_ids(reply: &str) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(reply).unwrap();
        v["tokens"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
    }
    fn page_next(reply: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(reply).unwrap();
        v["next"].as_str().unwrap().to_string()
    }
    fn token(id: &str) -> serde_json::Value {
        serde_json::from_str(&run("get_token", &addr('f'), &[id], 0).unwrap()).unwrap()
    }

    #[test]
    fn mint_creates_a_token_and_indexes_it() {
        setup();
        let a = addr('a');
        assert_eq!(mint_as(&a), "1");
        assert_eq!(run("total_supply", &a, &[], 0).unwrap(), "1");
        let t = token("1");
        assert_eq!(t["owner"], a);
        assert_eq!(t["creator"], a);
        assert_eq!(t["name"], "Cat");
        assert_eq!(t["image_uri"], "ipfs://bafy123");
        assert_eq!(t["image_hash"], hash());
        assert_eq!(t["is_listed"], false);
        assert_eq!(t["price"], "0");
        let mine = run("tokens_by_owner", &a, &[&a], 0).unwrap();
        assert_eq!(page_ids(&mine), vec!["1"]);
    }

    #[test]
    fn ids_are_sequential_and_canonical() {
        setup();
        let a = addr('a');
        assert_eq!(mint_as(&a), "1");
        assert_eq!(mint_as(&a), "2");
        assert_eq!(token("02")["id"], "2");
        assert!(run("get_token", &a, &["0"], 0).is_err());
        assert!(run("get_token", &a, &["x"], 0).is_err());
        assert!(run("get_token", &a, &["3"], 0).is_err());
    }

    #[test]
    fn mint_validation() {
        setup();
        let a = addr('a');
        let long_name = "n".repeat(49);
        let long_uri = format!("https://{}", "a".repeat(250));
        let bad_hash_upper = "A".repeat(64);
        let bad_hash_short = "a".repeat(63);
        let h = hash();
        let cases: Vec<[&str; 3]> = vec![
            ["", "ipfs://x", &h],
            [&long_name, "ipfs://x", &h],
            [" Cat", "ipfs://x", &h],
            ["Cat ", "ipfs://x", &h],
            ["Ca\u{0007}t", "ipfs://x", &h],
            ["Cat", "http://x", &h],
            ["Cat", "javascript:alert(1)", &h],
            ["Cat", "data:text/html,hi", &h],
            ["Cat", "ipfs://", &h],
            ["Cat", "ipfs://a b", &h],
            ["Cat", &long_uri, &h],
            ["Cat", "ipfs://x", &bad_hash_upper],
            ["Cat", "ipfs://x", &bad_hash_short],
        ];
        for case in cases {
            assert!(run("mint", &a, &case, 0).is_err(), "should reject {case:?}");
        }
        assert!(run("mint", &a, &["Cat", "ipfs://x"], 0).is_err());
        for uri in ["https://example.com/a.png", "ipfs://bafy", "ar://abc"] {
            assert!(run("mint", &a, &["Cat", uri, &h], 0).is_ok(), "should accept {uri}");
        }
        assert_eq!(run("total_supply", &a, &[], 0).unwrap(), "3");
    }

    #[test]
    fn only_buy_is_payable() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        let h = hash();
        assert!(run("mint", &a, &["Cat", "ipfs://x", &h], 1).is_err());
        assert!(run("transfer", &a, &["1", &b], 1).is_err());
        assert!(run("list", &a, &["1", "100"], 1).is_err());
        assert!(run("unlist", &a, &["1"], 1).is_err());
        assert!(run("get_token", &a, &["1"], 1).is_err());
        assert!(run("total_supply", &a, &[], 1).is_err());
        assert!(run("get_config", &a, &[], 1).is_err());
    }

    #[test]
    fn transfer_moves_owner_and_indexes_and_clears_listing() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        mint_as(&a);
        mint_as(&a);
        run_with_rollback("list", &a, &["2", "5"], 0).unwrap();
        assert!(run("transfer", &b, &["2", &b], 0).is_err(), "non-owner");
        assert!(run("transfer", &a, &["2", &a], 0).is_err(), "self");
        assert!(run("transfer", &a, &["2", "bob"], 0).is_err(), "bad address");
        assert!(run("transfer", &a, &["9", &b], 0).is_err(), "unknown token");
        run_with_rollback("transfer", &a, &["2", &b], 0).unwrap();
        assert_eq!(token("2")["owner"], b);
        assert_eq!(token("2")["is_listed"], false);
        assert_eq!(page_ids(&run("tokens_by_owner", &a, &[&a], 0).unwrap()), vec!["1", "3"]);
        assert_eq!(page_ids(&run("tokens_by_owner", &a, &[&b], 0).unwrap()), vec!["2"]);
        assert!(page_ids(&run("listed_tokens", &a, &[], 0).unwrap()).is_empty());
    }

    #[test]
    fn list_validation_and_price_update() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        assert!(run("list", &b, &["1", "100"], 0).is_err(), "non-owner");
        assert!(run("list", &a, &["1", "0"], 0).is_err(), "zero price");
        assert!(run("list", &a, &["1", "abc"], 0).is_err(), "bad price");
        assert!(run("list", &a, &["1", "-5"], 0).is_err(), "negative price");
        assert!(run("list", &a, &["7", "100"], 0).is_err(), "unknown token");
        run_with_rollback("list", &a, &["1", "100"], 0).unwrap();
        run_with_rollback("list", &a, &["1", "250"], 0).unwrap();
        assert_eq!(token("1")["price"], "250");
        assert_eq!(page_ids(&run("listed_tokens", &a, &[], 0).unwrap()), vec!["1"]);
        assert!(run("unlist", &b, &["1"], 0).is_err());
        run_with_rollback("unlist", &a, &["1"], 0).unwrap();
        assert_eq!(token("1")["is_listed"], false);
        assert!(run("unlist", &a, &["1"], 0).is_err(), "already unlisted");
    }

    #[test]
    fn buy_pays_the_seller_directly_and_refunds_the_excess() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        let price = 2 * RTM;
        let paid = price + RTM / 10;
        run_with_rollback("list", &a, &["1", &price.to_string()], 0).unwrap();
        run_with_rollback("buy", &b, &["1", &price.to_string()], paid).unwrap();

        assert_eq!(token("1")["owner"], b);
        assert_eq!(token("1")["is_listed"], false);
        assert!(page_ids(&run("listed_tokens", &a, &[], 0).unwrap()).is_empty());
        assert!(page_ids(&run("tokens_by_owner", &a, &[&a], 0).unwrap()).is_empty());
        assert_eq!(page_ids(&run("tokens_by_owner", &a, &[&b], 0).unwrap()), vec!["1"]);

        let calls = test_host::calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(calls[0].method, "transfer");
        assert_eq!(calls[0].args, [a.clone(), price.to_string()], "seller is paid directly");
        assert_eq!(calls[1].args, [b.clone(), (paid - price).to_string()], "excess goes back");
    }

    #[test]
    fn buy_with_exact_payment_sends_no_refund() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        run_with_rollback("list", &a, &["1", "1000"], 0).unwrap();
        run_with_rollback("buy", &b, &["1", "1000"], 1000).unwrap();
        let calls = test_host::calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].args, [a.clone(), "1000".to_string()]);
    }

    #[test]
    fn buy_rejects_bad_requests_and_changes_nothing() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        mint_as(&a);
        run_with_rollback("list", &a, &["1", "1000"], 0).unwrap();
        assert!(run("buy", &b, &["1", "999"], 5000).is_err(), "price above max_price");
        assert!(run("buy", &b, &["1", "1000"], 999).is_err(), "underpayment");
        assert!(run("buy", &b, &["1", "1000"], 0).is_err(), "no payment");
        assert!(run("buy", &a, &["1", "1000"], 1000).is_err(), "own token");
        assert!(run("buy", &b, &["2", "1000"], 1000).is_err(), "not listed");
        assert!(run("buy", &b, &["9", "1000"], 1000).is_err(), "unknown token");
        assert!(run("buy", &b, &["1"], 1000).is_err(), "missing max_price");
        assert!(run("buy", &b, &["1", "x"], 1000).is_err(), "bad max_price");
        assert_eq!(token("1")["owner"], a);
        assert_eq!(token("1")["price"], "1000");
        assert!(test_host::calls().is_empty());
    }

    #[test]
    fn a_price_raised_just_before_the_buy_is_refused() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        run_with_rollback("list", &a, &["1", "1000"], 0).unwrap();
        // Buyer sempat lihat harga 1000 dan tanda tangan dengan max_price 1000.
        run_with_rollback("list", &a, &["1", "9000"], 0).unwrap();
        assert!(run("buy", &b, &["1", "1000"], 9000).is_err());
        assert_eq!(token("1")["owner"], a);
    }

    #[test]
    fn an_rtm_error_reply_aborts_the_whole_buy() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        mint_as(&a);
        run_with_rollback("list", &a, &["1", "1000"], 0).unwrap();
        test_host::set_call_reply(call::RTM, "transfer", r#"{"ok":false,"error":"rejected"}"#);

        let err = run_with_rollback("buy", &b, &["1", "1000"], 1000).unwrap_err();
        assert!(err.contains("trapped"), "{err}");
        assert_eq!(token("1")["owner"], a, "owner is restored");
        assert_eq!(token("1")["price"], "1000", "listing is restored");
        assert_eq!(page_ids(&run("listed_tokens", &a, &[], 0).unwrap()), vec!["1"]);
        assert_eq!(page_ids(&run("tokens_by_owner", &a, &[&a], 0).unwrap()), vec!["1"]);
        assert!(page_ids(&run("tokens_by_owner", &a, &[&b], 0).unwrap()).is_empty());
    }

    #[test]
    fn payout_plus_refund_always_equals_the_attached_value() {
        let prices: [u128; 5] = [1, 7, 1_000, RTM, 123 * RTM + 17];
        let extras: [u128; 5] = [0, 1, 999, RTM / 3, 7 * RTM];
        for price in prices {
            for extra in extras {
                setup();
                let (a, b) = (addr('a'), addr('b'));
                mint_as(&a);
                run_with_rollback("list", &a, &["1", &price.to_string()], 0).unwrap();
                let paid = price + extra;
                run_with_rollback("buy", &b, &["1", &price.to_string()], paid).unwrap();
                let sum: u128 = test_host::calls().iter().map(|c| c.args[1].parse::<u128>().unwrap()).sum();
                assert_eq!(sum, paid, "price {price} extra {extra}");
            }
        }
    }

    #[test]
    fn the_new_owner_can_relist_after_a_sale() {
        setup();
        let (a, b, c) = (addr('a'), addr('b'), addr('c'));
        mint_as(&a);
        run_with_rollback("list", &a, &["1", "100"], 0).unwrap();
        run_with_rollback("buy", &b, &["1", "100"], 100).unwrap();
        assert!(run("list", &a, &["1", "100"], 0).is_err(), "old owner cannot list");
        run_with_rollback("list", &b, &["1", "300"], 0).unwrap();
        run_with_rollback("buy", &c, &["1", "300"], 300).unwrap();
        assert_eq!(token("1")["owner"], c);
        assert_eq!(test_host::calls().last().unwrap().args, [b.clone(), "300".to_string()]);
    }

    #[test]
    fn listing_index_stays_consistent_after_removals() {
        setup();
        let a = addr('a');
        for _ in 0..4 {
            mint_as(&a);
        }
        for id in ["1", "2", "3", "4"] {
            run_with_rollback("list", &a, &[id, "10"], 0).unwrap();
        }
        let listed = |_: ()| page_ids(&run("listed_tokens", &a, &[], 0).unwrap());
        assert_eq!(listed(()), vec!["1", "2", "3", "4"]);
        run_with_rollback("unlist", &a, &["2"], 0).unwrap(); // tengah
        assert_eq!(listed(()), vec!["1", "3", "4"]);
        run_with_rollback("unlist", &a, &["1"], 0).unwrap(); // pertama
        assert_eq!(listed(()), vec!["3", "4"]);
        run_with_rollback("unlist", &a, &["4"], 0).unwrap(); // terakhir
        assert_eq!(listed(()), vec!["3"]);
        run_with_rollback("unlist", &a, &["3"], 0).unwrap();
        assert!(listed(()).is_empty());
        run_with_rollback("list", &a, &["2", "10"], 0).unwrap();
        run_with_rollback("list", &a, &["1", "10"], 0).unwrap();
        assert_eq!(listed(()), vec!["2", "1"], "relisted tokens go to the end");
    }

    #[test]
    fn paging_walks_every_token_once() {
        setup();
        let a = addr('a');
        for _ in 0..5 {
            mint_as(&a);
        }
        for id in ["1", "2", "3", "4", "5"] {
            run_with_rollback("list", &a, &[id, "10"], 0).unwrap();
        }
        let mut seen = Vec::new();
        let mut after = String::new();
        for _ in 0..10 {
            let reply = run("listed_tokens", &a, &[&after, "2"], 0).unwrap();
            seen.extend(page_ids(&reply));
            after = page_next(&reply);
            if after.is_empty() {
                break;
            }
        }
        assert_eq!(seen, vec!["1", "2", "3", "4", "5"]);
        let owned = run("tokens_by_owner", &a, &[&a, "", "3"], 0).unwrap();
        assert_eq!(page_ids(&owned), vec!["1", "2", "3"]);
        assert_eq!(page_next(&owned), "3");
        assert!(run("tokens_by_owner", &a, &["bob"], 0).is_err());
        assert!(run("listed_tokens", &a, &["zzz"], 0).is_err());
    }

    #[test]
    fn a_cursor_on_a_removed_token_can_still_move_forward() {
        setup();
        let (a, b) = (addr('a'), addr('b'));
        for _ in 0..3 {
            mint_as(&a);
        }
        for id in ["1", "2", "3"] {
            run_with_rollback("list", &a, &[id, "10"], 0).unwrap();
        }
        let first = run("listed_tokens", &a, &["", "1"], 0).unwrap();
        assert_eq!(page_ids(&first), vec!["1"]);
        assert_eq!(page_next(&first), "1");
        // Token 1 keburu terjual di antara dua request page.
        run_with_rollback("buy", &b, &["1", "10"], 10).unwrap();
        let second = run("listed_tokens", &a, &["1", "5"], 0).unwrap();
        assert_eq!(page_ids(&second), vec!["2", "3"]);
    }

    #[test]
    fn config_and_unknown_actions() {
        setup();
        let a = addr('a');
        let config = run("get_config", &a, &[], 0).unwrap();
        assert!(config.contains("v0.1.0"), "{config}");
        assert!(run("nope", &a, &[], 0).is_err());
        assert_eq!(run("total_supply", &a, &[], 0).unwrap(), "0");
    }

    #[test]
    fn address_and_hash_checks_are_strict() {
        assert!(is_valid_address(&addr('a')));
        assert!(!is_valid_address("rete"));
        assert!(!is_valid_address(&format!("{}g", &addr('a')[..71])));
        assert!(!is_valid_address(&format!("{}A", &addr('a')[..71])));
        assert!(!is_valid_address(&format!("retf{}", &addr('a')[4..])));
        assert!(!is_valid_address(&format!("{}0", addr('a'))));
        assert!(is_sha256_hex(&hash()));
        assert!(!is_sha256_hex(&"g".repeat(64)));
    }
}