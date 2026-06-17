use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use surrealdb::{
    engine::local::{Db, SurrealKv},
    Surreal,
};
use tauri::{Manager, Runtime, State};
use thiserror::Error;
use tokio::sync::Mutex;

const SAMPLE_COUNT: usize = 4;

#[derive(Debug, Error)]
enum AppError {
    #[error("{0}")]
    User(String),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("tauri error: {0}")]
    Tauri(#[from] tauri::Error),
}

impl From<AppError> for String {
    fn from(value: AppError) -> Self {
        value.to_string()
    }
}

struct AppState {
    db: Surreal<Db>,
    io: Arc<Mutex<()>>,
    db_dir: PathBuf,
    cache_dir: PathBuf,
}

#[cfg(target_os = "android")]
struct WcnMobilePlugin<R: Runtime>(tauri::plugin::PluginHandle<R>);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WifiScanResponse {
    sample_count: usize,
    scan_throttle_enabled: Option<bool>,
    samples: Vec<WifiScanSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WifiScanSample {
    index: usize,
    networks: Vec<WifiNetworkReading>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WifiNetworkReading {
    ssid: String,
    bssid: String,
    level: i32,
    frequency: Option<i32>,
    timestamp_micros: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FingerprintNetwork {
    bssid: String,
    ssid: String,
    rssi_dbm: Vec<Option<i32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FingerprintRecord {
    pose: String,
    cell: String,
    orientation: String,
    updated_at: String,
    sample_count: usize,
    networks: Vec<FingerprintNetwork>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintSummary {
    pose: String,
    cell: String,
    orientation: String,
    updated_at: String,
    sample_count: usize,
    network_count: usize,
    scan_throttle_enabled: Option<bool>,
    database_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupResult {
    filename: String,
    bytes: u64,
    uri: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg(target_os = "android")]
struct ScanSamplesArgs {
    sample_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg(target_os = "android")]
struct SaveBackupArgs {
    source_path: String,
    suggested_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(target_os = "android")]
struct SaveBackupResponse {
    uri: String,
    bytes: u64,
}

#[tauri::command]
async fn start_fingerprinting(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    cell: String,
    orientation: String,
) -> Result<FingerprintSummary, String> {
    start_fingerprinting_inner(app, &state, cell, orientation)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn save_database_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<BackupResult, String> {
    save_database_backup_inner(app, &state)
        .await
        .map_err(Into::into)
}

async fn start_fingerprinting_inner(
    app: tauri::AppHandle,
    state: &AppState,
    cell: String,
    orientation: String,
) -> Result<FingerprintSummary, AppError> {
    let cell = normalize_cell(&cell)?;
    let orientation = normalize_orientation(&orientation)?;
    let pose = format!("{cell}{orientation}");

    let scan_response = scan_wifi_samples(&app, SAMPLE_COUNT).await?;
    if scan_response.samples.len() != SAMPLE_COUNT {
        return Err(AppError::User(format!(
            "expected {SAMPLE_COUNT} fresh scans, got {}",
            scan_response.samples.len()
        )));
    }

    let networks = aggregate_networks(&scan_response.samples, SAMPLE_COUNT);
    let updated_at = Utc::now().to_rfc3339();
    let record = FingerprintRecord {
        pose: pose.clone(),
        cell: cell.clone(),
        orientation: orientation.clone(),
        updated_at: updated_at.clone(),
        sample_count: SAMPLE_COUNT,
        networks,
    };
    let network_count = record.networks.len();

    {
        let _guard = state.io.lock().await;
        let value = serde_json::to_value(record)
            .map_err(|error| AppError::User(format!("could not serialize fingerprint: {error}")))?;
        let _: Option<serde_json::Value> = state
            .db
            .upsert(("Fingerprints", pose.clone()))
            .content(value)
            .await?;
    }

    Ok(FingerprintSummary {
        pose,
        cell,
        orientation,
        updated_at,
        sample_count: SAMPLE_COUNT,
        network_count,
        scan_throttle_enabled: scan_response.scan_throttle_enabled,
        database_path: state.db_dir.display().to_string(),
    })
}

async fn save_database_backup_inner(
    app: tauri::AppHandle,
    state: &AppState,
) -> Result<BackupResult, AppError> {
    let filename = format!(
        "wcn-fingerprints-{}.surql",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let export_dir = state.cache_dir.join("exports");
    fs::create_dir_all(&export_dir)?;
    let temp_path = export_dir.join(&filename);
    remove_if_exists(&temp_path)?;

    {
        let _guard = state.io.lock().await;
        state.db.export(&temp_path).await?;
    }

    let result = save_exported_file(&app, &temp_path, &filename).await;
    let _ = fs::remove_file(&temp_path);
    result
}

#[cfg(target_os = "android")]
async fn scan_wifi_samples(
    app: &tauri::AppHandle,
    sample_count: usize,
) -> Result<WifiScanResponse, AppError> {
    let plugin = app.state::<WcnMobilePlugin<tauri::Wry>>();
    let response = plugin
        .0
        .run_mobile_plugin_async("scanWifiSamples", ScanSamplesArgs { sample_count })
        .await
        .map_err(|error| AppError::User(format!("mobile bridge error: {error}")))?;
    Ok(response)
}

#[cfg(not(target_os = "android"))]
async fn scan_wifi_samples(
    _app: &tauri::AppHandle,
    _sample_count: usize,
) -> Result<WifiScanResponse, AppError> {
    Err(AppError::User(
        "Wi-Fi fingerprinting requires a physical Android device".to_string(),
    ))
}

#[cfg(target_os = "android")]
async fn save_exported_file(
    app: &tauri::AppHandle,
    temp_path: &Path,
    filename: &str,
) -> Result<BackupResult, AppError> {
    let plugin = app.state::<WcnMobilePlugin<tauri::Wry>>();
    let response: SaveBackupResponse = plugin
        .0
        .run_mobile_plugin_async(
            "saveBackup",
            SaveBackupArgs {
                source_path: temp_path.display().to_string(),
                suggested_name: filename.to_string(),
            },
        )
        .await
        .map_err(|error| AppError::User(format!("mobile bridge error: {error}")))?;

    Ok(BackupResult {
        filename: filename.to_string(),
        bytes: response.bytes,
        uri: Some(response.uri),
        path: None,
    })
}

#[cfg(not(target_os = "android"))]
async fn save_exported_file(
    app: &tauri::AppHandle,
    temp_path: &Path,
    filename: &str,
) -> Result<BackupResult, AppError> {
    let backup_dir = app.path().app_data_dir()?.join("backups");
    fs::create_dir_all(&backup_dir)?;
    let destination = backup_dir.join(filename);
    let bytes = fs::copy(temp_path, &destination)?;
    Ok(BackupResult {
        filename: filename.to_string(),
        bytes,
        uri: None,
        path: Some(destination.display().to_string()),
    })
}

fn aggregate_networks(samples: &[WifiScanSample], sample_count: usize) -> Vec<FingerprintNetwork> {
    let mut map: BTreeMap<String, FingerprintNetwork> = BTreeMap::new();

    for sample in samples {
        if sample.index >= sample_count {
            continue;
        }

        for network in sample
            .networks
            .iter()
            .filter(|network| matches_allowed_ssid(&network.ssid))
        {
            let entry = map
                .entry(network.bssid.clone())
                .or_insert_with(|| FingerprintNetwork {
                    bssid: network.bssid.clone(),
                    ssid: network.ssid.clone(),
                    rssi_dbm: vec![None; sample_count],
                });

            entry.ssid = network.ssid.clone();
            entry.rssi_dbm[sample.index] = Some(network.level);
        }
    }

    map.into_values().collect()
}

fn matches_allowed_ssid(ssid: &str) -> bool {
    ssid == "IN-foo" || ssid == "eduroam" || ssid.to_ascii_lowercase().contains("hfu")
}

fn normalize_cell(cell: &str) -> Result<String, AppError> {
    let value = cell.trim().to_ascii_lowercase();
    let bytes = value.as_bytes();
    if bytes.len() == 2 && (b'a'..=b'e').contains(&bytes[0]) && (b'0'..=b'5').contains(&bytes[1]) {
        Ok(value)
    } else {
        Err(AppError::User(format!(
            "invalid cell '{cell}', expected a0 through e5"
        )))
    }
}

fn normalize_orientation(orientation: &str) -> Result<String, AppError> {
    let value = orientation.trim().to_ascii_lowercase();
    match value.as_str() {
        "u" | "d" | "l" | "r" => Ok(value),
        _ => Err(AppError::User(format!(
            "invalid orientation '{orientation}', expected u, d, l, or r"
        ))),
    }
}

fn remove_if_exists(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn initialize_state(app: &tauri::AppHandle) -> Result<AppState, AppError> {
    let app_data_dir = app.path().app_data_dir()?;
    let cache_dir = app.path().app_cache_dir()?;
    let db_dir = app_data_dir.join("surrealdb");
    fs::create_dir_all(&db_dir)?;
    fs::create_dir_all(&cache_dir)?;

    let db = Surreal::new::<SurrealKv>(db_dir.as_path()).await?;
    db.use_ns("wcn").use_db("fingerprinting").await?;
    db.query("DEFINE TABLE IF NOT EXISTS Fingerprints SCHEMALESS;")
        .await?;

    Ok(AppState {
        db,
        io: Arc::new(Mutex::new(())),
        db_dir,
        cache_dir,
    })
}

fn init_wcn_mobile_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("wcn")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin("de.hfu.wcnfingerprinting", "WcnPlugin")?;
                _app.manage(WcnMobilePlugin(handle));
            }
            Ok(())
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(init_wcn_mobile_plugin())
        .setup(|app| {
            let state = tauri::async_runtime::block_on(initialize_state(app.handle()))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_fingerprinting,
            save_database_backup
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_cells_and_orientations() {
        assert_eq!(normalize_cell("A0").unwrap(), "a0");
        assert_eq!(normalize_cell("e5").unwrap(), "e5");
        assert!(normalize_cell("f0").is_err());
        assert!(normalize_cell("a6").is_err());

        assert_eq!(normalize_orientation("U").unwrap(), "u");
        assert!(normalize_orientation("north").is_err());
    }

    #[test]
    fn filters_ssids() {
        assert!(matches_allowed_ssid("IN-foo"));
        assert!(matches_allowed_ssid("eduroam"));
        assert!(matches_allowed_ssid("HFU-wlan"));
        assert!(matches_allowed_ssid("campus-hfu-lab"));
        assert!(!matches_allowed_ssid("guest"));
    }

    #[test]
    fn aggregates_missing_samples_as_nulls() {
        let samples = vec![
            WifiScanSample {
                index: 0,
                networks: vec![WifiNetworkReading {
                    ssid: "eduroam".into(),
                    bssid: "aa:bb:cc:dd:ee:ff".into(),
                    level: -61,
                    frequency: Some(2412),
                    timestamp_micros: Some(1),
                }],
            },
            WifiScanSample {
                index: 2,
                networks: vec![WifiNetworkReading {
                    ssid: "eduroam".into(),
                    bssid: "aa:bb:cc:dd:ee:ff".into(),
                    level: -65,
                    frequency: Some(2412),
                    timestamp_micros: Some(2),
                }],
            },
        ];

        let networks = aggregate_networks(&samples, 4);
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].rssi_dbm, vec![Some(-61), None, Some(-65), None]);
    }
}
