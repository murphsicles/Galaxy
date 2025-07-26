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
use std::collections::HashMap;
use tokio::time::{Instant, Duration};
use tokio::sync::Mutex;

pub struct TrackerManager {
    tracker: Tracker,
    config: Config,
    event_tx: mpsc::Sender<TrackerEvent>,
    reputation: Arc<Mutex<HashMap<String, Reputation>>>,
}

#[derive(Debug)]
pub enum TrackerEvent {
    SeederRegistered(String, String), // peer_id, info_hash
    SeederDropped(String, String),
}

#[derive(Debug)]
struct Reputation {
    uptime: Duration,
    score: u64, // Reputation score based on uptime and contributions
    last_seen: Instant,
}

impl TrackerManager {
    pub async fn new(config: &Config) -> Self {
        let tracker_config = TrackerConfig::default();
        tracker_config.port = config.tracker_port.unwrap_or(6969);
        tracker_config.private = true;

        let tracker = Tracker::new(&tracker_config)
            .await
            .expect("Failed to initialize tracker");
        let (event_tx, _) = mpsc::channel(32);

        Self {
            tracker,
            config: config.clone(),
            event_tx,
            reputation: Arc::new(Mutex::new(HashMap::new())),
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
        if !signature.verify(message, pub_key) {
            return Err(ServiceError::AuthError("Invalid BSV signature for seeder registration".to_string()));
        }

        // Check reputation to prevent sybil attacks
        let mut rep = self.reputation.lock().await;
        let rep_entry = rep.entry(peer_id.to_string()).or_insert(Reputation {
            uptime: Duration::from_secs(0),
            score: 0,
            last_seen: Instant::now(),
        });
        if rep_entry.score < 100 { // Arbitrary threshold for new peers
            return Err(ServiceError::Torrent("Insufficient reputation for seeder registration".to_string()));
        }

        self.tracker
            .add_peer(peer_id, info_hash, true)
            .await
            .map_err(|e| ServiceError::Torrent(format!("Failed to register seeder: {}", e)))?;
        info!("Registered seeder {} for info_hash {} with valid BSV signature", peer_id, info_hash);

        self.event_tx.send(TrackerEvent::SeederRegistered(peer_id.to_string(), info_hash.to_string())).await
            .map_err(|e| ServiceError::Torrent(format!("Failed to send event: {}", e)))?;
        Ok(())
    }

    pub async fn update_reputation(&self, peer_id: &str, delta: u64) -> Result<(), ServiceError> {
        let mut rep = self.reputation.lock().await;
        if let Some(entry) = rep.get_mut(peer_id) {
            entry.uptime += entry.last_seen.elapsed();
            entry.last_seen = Instant::now();
            entry.score += delta;
            info!("Updated reputation for peer {}: score = {}", peer_id, entry.score);
        }
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
        info!("Starting tracker on port {}", self.config.tracker_port.unwrap_or(6969));
        if let Err(e) = self.tracker.run().await {
            error!("Tracker error: {}", e);
        }
    }
}
