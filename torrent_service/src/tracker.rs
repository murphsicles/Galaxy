// torrent_service/src/tracker.rs
use crate::utils::{Config, ServiceError};
use torrust_tracker::Tracker;
use torrust_tracker::config::Configuration as TrackerConfig;
use tokio::sync::mpsc;
use tracing::{info, error};
use bip_metainfo::Metainfo;
use std::sync::Arc;
use sv::keys::PublicKey;
use sv::signature::{Signature, Message};
use sv::transaction::Transaction as SvTx;  // For potential future use

pub struct TrackerManager {
    tracker: Tracker,
    config: Config,
    event_tx: mpsc::Sender<TrackerEvent>,
}

#[derive(Debug)]
pub enum TrackerEvent {
    SeederRegistered(String, String), // peer_id, info_hash
    SeederDropped(String, String),
}

impl TrackerManager {
    pub async fn new(config: &Config) -> Self {
        let mut tracker_config = TrackerConfig::default();
        tracker_config.port = config.tracker_port.unwrap_or(6969);
        tracker_config.private = true; // Enforce private mode

        let tracker = Tracker::new(&tracker_config)
            .await
            .expect("Failed to initialize tracker");
        let (event_tx, _) = mpsc::channel(32);

        Self {
            tracker,
            config: config.clone(),
            event_tx,
        }
    }

    pub async fn announce(&self, torrent: &Metainfo) -> Result<(), ServiceError> {
        self.tracker
            .add_torrent(torrent)
            .await
            .map_err(|e| ServiceError::Torrent(format!("Failed to announce torrent: {}", e)))?;
        info!("Announced torrent with info_hash: {}", torrent.info_hash());
        Ok(())
    }

    pub async fn register_seeder(&self, peer_id: &str, info_hash: &str, signature: &Signature, message: &Message, pub_key: &PublicKey) -> Result<(), ServiceError> {
        // Verify BSV wallet signature for authentication
        if !signature.verify(message, pub_key) {
            return Err(ServiceError::AuthError("Invalid BSV signature for seeder registration".to_string()));
        }

        // Add seeder to tracker
        self.tracker
            .add_peer(peer_id, info_hash, true)  // true for seeder
            .await
            .map_err(|e| ServiceError::Torrent(format!("Failed to register seeder: {}", e)))?;
        info!("Registered seeder {} for info_hash {} with valid BSV signature", peer_id, info_hash);

        self.event_tx.send(TrackerEvent::SeederRegistered(peer_id.to_string(), info_hash.to_string())).await
            .map_err(|e| ServiceError::Torrent(format!("Failed to send event: {}", e)))?;
        Ok(())
    }

    pub async fn get_seeders(&self, info_hash: &str) -> Result<Vec<String>, ServiceError> {
        let peers = self.tracker
            .get_peers(info_hash)
            .await
            .map_err(|e| ServiceError::Torrent(format!("Failed to fetch peers: {}", e)))?;
        let seeder_addrs: Vec<String> = peers
            .into_iter()
            .filter(|peer| peer.is_seeder)
            .map(|peer| peer.address.to_string())
            .collect();
        Ok(seeder_addrs)
    }

    pub async fn run(&self) {
        // Start tracker loop
        info!("Starting tracker on port {}", self.config.tracker_port.unwrap_or(6969));
        if let Err(e) = self.tracker.run().await {
            error!("Tracker error: {}", e);
        }
    }
}
