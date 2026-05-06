//! PNGs copied from `installers/linux/files` and baked into the binary via `include_bytes!`.
//! Electrum-LTC asset must stay in sync with `installers/linux/files/electrum-ltc.png`.

static ELECTRUM_BTC: &[u8] = include_bytes!("../assets/catalog-icons/electrum-btc.png");
static ELECTRUM_LTC: &[u8] = include_bytes!("../assets/catalog-icons/electrum-ltc.png");
static FEATHER: &[u8] = include_bytes!("../assets/catalog-icons/feather.png");
static SESSION: &[u8] = include_bytes!("../assets/catalog-icons/session.png");
static KEEPASSXC: &[u8] = include_bytes!("../assets/catalog-icons/keepassxc.png");
static MATTERMOST: &[u8] = include_bytes!("../assets/catalog-icons/mattermost.png");
static ANYTHINGLLM: &[u8] = include_bytes!("../assets/catalog-icons/anythingllm.png");
static JAN: &[u8] = include_bytes!("../assets/catalog-icons/jan.png");
static LM_STUDIO: &[u8] = include_bytes!("../assets/catalog-icons/lm-studio.png");
static CRYPTOMATOR: &[u8] = include_bytes!("../assets/catalog-icons/cryptomator.png");
/// Official Linux icon set (laurent22/joplin Assets/LinuxIcons/256x256.png) — sharp at list size.
static JOPLIN: &[u8] = include_bytes!("../assets/catalog-icons/joplin.png");
static STANDARD_NOTES: &[u8] = include_bytes!("../assets/catalog-icons/standard-notes.png");
static ZULIP: &[u8] = include_bytes!("../assets/catalog-icons/zulip.png");

pub fn embedded_icon_png(app_id: &str) -> Option<&'static [u8]> {
    match app_id {
        "anythingllm" => Some(ANYTHINGLLM),
        "cryptomator" => Some(CRYPTOMATOR),
        "electrum-btc" => Some(ELECTRUM_BTC),
        "electrum-ltc" => Some(ELECTRUM_LTC),
        "feather" => Some(FEATHER),
        "joplin" => Some(JOPLIN),
        "jan" => Some(JAN),
        "keepassxc" => Some(KEEPASSXC),
        "lm-studio" => Some(LM_STUDIO),
        "mattermost" => Some(MATTERMOST),
        "session" => Some(SESSION),
        "standard-notes" => Some(STANDARD_NOTES),
        "zulip" => Some(ZULIP),
        _ => None,
    }
}
