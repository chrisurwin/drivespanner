mod api;
mod config;
mod core;
mod logger;
mod service;
mod vfs;

use api::ApiServer;
use config::{PoolConfig, SharedConfig};
use core::pool::StoragePool;
use log::{error, info, warn};
use service::ServiceManager;
use vfs::PoolMounter;

use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;

fn main() {
    logger::init_logger();

    let args: Vec<String> = env::args().collect();
    let command = if args.len() > 1 {
        args[1].to_lowercase()
    } else {
        "run".to_string()
    };

    match command.as_str() {
        "help" | "-h" | "--help" => {
            print_banner();
            print_usage();
        }
        "service" => {
            let subcmd = args.get(2).map(|s| s.as_str()).unwrap_or("help");
            match subcmd {
                "install" => {
                    info!("Installing PoolForge Windows Service...");
                    if let Err(e) = ServiceManager::install() {
                        error!("Service install error: {}", e);
                    }
                }
                "uninstall" => {
                    info!("Uninstalling PoolForge Windows Service...");
                    if let Err(e) = ServiceManager::uninstall() {
                        error!("Service uninstall error: {}", e);
                    }
                }
                "start" => {
                    info!("Starting PoolForge Windows Service...");
                    if let Err(e) = ServiceManager::start() {
                        error!("Service start error: {}", e);
                    }
                }
                "stop" => {
                    info!("Stopping PoolForge Windows Service...");
                    if let Err(e) = ServiceManager::stop() {
                        error!("Service stop error: {}", e);
                    }
                }
                _ => {
                    println!("Usage: poolforge service [install | uninstall | start | stop]");
                }
            }
        }
        "set-mount" => {
            if args.len() < 3 {
                println!("Usage: poolforge set-mount <LETTER>");
                return;
            }
            let raw = &args[2];
            let mut letter = raw.trim().to_uppercase();
            if letter.len() == 1 {
                letter.push(':');
            }
            if letter.len() != 2 || !letter.ends_with(':') {
                eprintln!("Invalid drive letter: '{}'. Please use format like 'V:'", raw);
                return;
            }
            let config_path = PoolConfig::get_config_path();
            let mut cfg = PoolConfig::load_or_default(&config_path);
            cfg.mount_point = letter.clone();
            match cfg.save_to(&config_path) {
                Ok(_) => println!("Successfully updated virtual pool mount point to {}", letter),
                Err(e) => eprintln!("Failed to save configuration: {}", e),
            }
        }
        "status" => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio runtime");

            rt.block_on(async {
                print_banner();
                let config_path = PoolConfig::get_config_path();
                let cfg = PoolConfig::load_or_default(&config_path);
                let shared_cfg: SharedConfig = Arc::new(RwLock::new(cfg));
                let (pool, _rep_handle) = StoragePool::new(shared_cfg);
                let _ = pool.initialize_from_config().await;

                let overview = pool.get_overview_stats().await;
                println!("  POOL STATUS");
                println!("  -------------------------------------------------------------");
                println!("  Pool ID:          {}", overview.pool_id);
                println!("  Pool Name:        {}", overview.pool_name);
                println!("  Mount Letter:     {}", overview.mount_point);
                println!("  Status Badge:     {}", overview.status_badge);
                println!(
                    "  Capacity:         {:.2} GB Total | {:.2} GB Free | {:.2} GB Used ({:.1}%)",
                    overview.total_bytes as f64 / 1_073_741_824.0,
                    overview.free_bytes as f64 / 1_073_741_824.0,
                    overview.used_bytes as f64 / 1_073_741_824.0,
                    overview.used_percent
                );
                println!("  Member Drives:    {}", overview.member_drive_count);
                println!("  Total Files:      {}", overview.total_files_count);
                println!("  Under-Replicated: {}", overview.under_replicated_count);
                println!("  Imbalance:        {:.1}%", overview.pool_imbalance_percent);
                println!("  -------------------------------------------------------------\n");

                let disks = pool.get_member_disks_stats().await;
                if disks.is_empty() {
                    println!("  No member drives configured. Add drives via:");
                    println!("  poolforge add D:\\\n");
                } else {
                    println!("  MEMBER DRIVES:");
                    for d in disks {
                        println!(
                            "  [{}] {} -> PoolData: {} | {:.2} GB free of {:.2} GB ({:.1}% used) [Online: {}]",
                            d.id.chars().take(8).collect::<String>(),
                            d.drive_path,
                            d.pooldata_path,
                            d.free_bytes as f64 / 1_073_741_824.0,
                            d.total_bytes as f64 / 1_073_741_824.0,
                            d.used_percent,
                            d.online
                        );
                    }
                    println!();
                }
            });
        }
        "add" => {
            if args.len() < 3 {
                println!("Usage: poolforge add <DRIVE_PATH> [--landing-zone]");
                return;
            }
            let drive_path = args[2].clone();
            let is_lz = args.iter().any(|a| a == "--landing-zone");

            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio runtime");

            rt.block_on(async move {
                let config_path = PoolConfig::get_config_path();
                let cfg = PoolConfig::load_or_default(&config_path);
                let shared_cfg: SharedConfig = Arc::new(RwLock::new(cfg));
                let (pool, _rep_handle) = StoragePool::new(shared_cfg);
                let _ = pool.initialize_from_config().await;

                match pool.add_member_drive(drive_path.clone(), is_lz).await {
                    Ok(_) => println!("Successfully added drive {} to storage pool.", drive_path),
                    Err(e) => eprintln!("Error adding drive {}: {}", drive_path, e),
                }
            });
        }
        "mount" => {
            let letter = args.get(2).cloned().unwrap_or_else(|| "V:".to_string());

            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio runtime");

            rt.block_on(async move {
                let config_path = PoolConfig::get_config_path();
                let cfg = PoolConfig::load_or_default(&config_path);
                let shared_cfg: SharedConfig = Arc::new(RwLock::new(cfg));
                let (pool, _rep_handle) = StoragePool::new(shared_cfg);
                let pool_arc = Arc::new(pool);
                let _ = pool_arc.initialize_from_config().await;

                info!("Starting WinFsp mount on {}", letter);
                match PoolMounter::start_mount(pool_arc, letter.clone()) {
                    Ok(_) => {
                        info!("WinFsp filesystem dispatcher initiated for {}", letter);
                        let _ = tokio::signal::ctrl_c().await;
                    }
                    Err(e) => error!("Mount error: {}", e),
                }
            });
        }
        "run" => {
            print_banner();
            info!("Starting PoolForge daemon in foreground...");
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio runtime");

            if let Err(e) = rt.block_on(async { async_main().await }) {
                error!("Daemon error: {}", e);
            }
        }
        unknown => {
            eprintln!("Unknown command: '{}'. Run 'poolforge help' for usage.", unknown);
        }
    }
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = PoolConfig::get_config_path();
    info!("Loading storage configuration from {}", config_path.display());
    let cfg = PoolConfig::load_or_default(&config_path);
    let mount_point = cfg.mount_point.clone();
    let shared_cfg: SharedConfig = Arc::new(RwLock::new(cfg));

    let (pool, _rep_handle) = StoragePool::new(shared_cfg.clone());
    let pool_arc = Arc::new(pool);

    pool_arc.initialize_from_config().await?;

    // Attempt WinFsp mount if mount point is configured
    let mount_pool_clone = pool_arc.clone();
    let mount_letter = mount_point.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        info!("Attempting to mount virtual storage on {}", mount_letter);
        match PoolMounter::start_mount(mount_pool_clone, mount_letter.clone()) {
            Ok(_) => info!("Pool successfully mounted on {}", mount_letter),
            Err(e) => warn!("WinFsp mount notice: {}", e),
        }
    });

    // Start API server and Web Dashboard
    let server_pool = pool_arc.clone();
    let api_task = tokio::spawn(async move {
        if let Err(e) = ApiServer::start(server_pool).await {
            error!("Web API server encountered error: {}", e);
        }
    });

    info!("PoolForge service is active. Web UI available at http://localhost:8989");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal. Exiting PoolForge...");
        }
        _ = api_task => {
            warn!("API server closed.");
        }
    }

    Ok(())
}

fn print_banner() {
    println!(r#"
  _____             _ ______                     
 |  __ \           | |  ____|                    
 | |__) |__   ___  | | |__ ___  _ __ __ _  ___   
 |  ___/ _ \ / _ \ | |  __/ _ \| '__/ _` |/ _ \  
 | |  | (_) | (_) || | | | (_) | | | (_| |  __/  
 |_|   \___/ \___/ |_|_|  \___/|_|  \__, |\___|  
                                     __/ |       
                                    |___/        
  High-Performance Storage Pool Daemon for Windows
  Folder Duplication - Drive Balancing - Zero Striping
"#);
}

fn print_usage() {
    println!(r#"
Usage: poolforge <COMMAND> [OPTIONS]

Commands:
  run                   Run the storage daemon in the foreground (default)
  status                Display current pool health, drives, and duplication status
  add <DRIVE_PATH>      Add a physical drive or folder to the storage pool
  mount [LETTER]        Mount the virtual filesystem drive letter (default: V:)
  set-mount <LETTER>    Update the virtual filesystem mount letter in config
  service install       Register PoolForge as an automatic Windows Service
  service uninstall     Remove the PoolForge Windows Service
  service start         Start the Windows Service
  service stop          Stop the Windows Service
  help                  Display this help message

Options:
  --landing-zone        When adding a drive, mark it as a high-speed landing zone

Web Dashboard:
  When running, access http://localhost:8989 in any browser.
"#);
}
