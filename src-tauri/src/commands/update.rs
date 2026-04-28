use serde::Serialize;
use std::fs::{self, File};
use std::path::PathBuf;
use std::time::Duration;
use tauri::AppHandle;

const GITHUB_OWNER: &str = "YiZhanSuDaShui";
const GITHUB_REPO: &str = "mini-todo";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAsset {
    pub name: String,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestReleaseInfo {
    pub tag_name: String,
    pub release_url: String,
    pub installer_asset: Option<UpdateAsset>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDownloadResult {
    pub file_path: String,
    pub file_name: String,
    pub bytes: u64,
}

fn release_latest_url() -> String {
    format!("https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest")
}

fn release_download_url(tag_name: &str, file_name: &str) -> String {
    format!(
        "https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases/download/{tag_name}/{file_name}"
    )
}

fn updater_client(timeout_secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("Mini Todo updater")
        .build()
        .map_err(|e| format!("创建更新请求客户端失败: {e}"))
}

fn parse_tag_from_release_url(url: &str) -> Option<String> {
    let marker = "/releases/tag/";
    let (_, tail) = url.split_once(marker)?;
    let tag = tail
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/');

    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

fn windows_installer_asset(tag_name: &str) -> Option<UpdateAsset> {
    #[cfg(target_os = "windows")]
    {
        let version = tag_name.trim_start_matches('v');
        let name = format!("mini-todo_{version}_x64-setup.exe");
        return Some(UpdateAsset {
            download_url: release_download_url(tag_name, &name),
            name,
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = tag_name;
        None
    }
}

fn ensure_safe_installer_request(download_url: &str, file_name: &str) -> Result<(), String> {
    let allowed_prefix =
        format!("https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases/download/");
    if !download_url.starts_with(&allowed_prefix) {
        return Err("安装包下载地址不属于 Mini Todo Release".to_string());
    }

    if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
        return Err("安装包文件名不安全".to_string());
    }

    if !file_name.ends_with("_x64-setup.exe") {
        return Err("当前只支持下载 Windows x64 安装包".to_string());
    }

    Ok(())
}

fn update_download_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("MiniTodoUpdates");
    fs::create_dir_all(&dir).map_err(|e| format!("创建更新下载目录失败: {e}"))?;
    Ok(dir)
}

#[tauri::command]
pub fn get_latest_release_info() -> Result<LatestReleaseInfo, String> {
    let client = updater_client(20)?;
    let response = client
        .get(release_latest_url())
        .send()
        .map_err(|e| format!("获取最新 Release 失败: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("GitHub Release 页面返回 {}", response.status()));
    }

    let release_url = response.url().to_string();
    let tag_name = parse_tag_from_release_url(&release_url)
        .ok_or_else(|| "未能从 GitHub Release 页面解析最新版本".to_string())?;

    Ok(LatestReleaseInfo {
        installer_asset: windows_installer_asset(&tag_name),
        release_url,
        tag_name,
    })
}

#[tauri::command]
pub fn download_update_installer(
    download_url: String,
    file_name: String,
) -> Result<UpdateDownloadResult, String> {
    ensure_safe_installer_request(&download_url, &file_name)?;

    let client = updater_client(600)?;
    let mut response = client
        .get(&download_url)
        .send()
        .map_err(|e| format!("下载安装包失败: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("安装包下载返回 {}", response.status()));
    }

    let file_path = update_download_dir()?.join(&file_name);
    let mut file = File::create(&file_path).map_err(|e| format!("创建安装包文件失败: {e}"))?;
    let bytes =
        std::io::copy(&mut response, &mut file).map_err(|e| format!("写入安装包失败: {e}"))?;

    Ok(UpdateDownloadResult {
        file_path: file_path.to_string_lossy().to_string(),
        file_name,
        bytes,
    })
}

#[tauri::command]
pub fn install_update_and_exit(
    app_handle: AppHandle,
    installer_path: String,
) -> Result<(), String> {
    let path = PathBuf::from(&installer_path);
    if !path.exists() {
        return Err("安装包不存在，请重新下载".to_string());
    }

    if path.extension().and_then(|value| value.to_str()) != Some("exe") {
        return Err("当前只支持启动 Windows 安装包".to_string());
    }

    std::process::Command::new(&path)
        .spawn()
        .map_err(|e| format!("启动安装程序失败: {e}"))?;
    app_handle.exit(0);
    Ok(())
}
