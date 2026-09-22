use super::{config::Config, storage::{Store,private_file}, Result};
use axum::{extract::{State,Path,DefaultBodyLimit}, http::StatusCode, routing::{get,post}, Json, Router};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{sync::{Arc,Mutex},time::{Duration,Instant},fs};
use tokio::sync::{Semaphore,RwLock};
use flux_p2p::{NetworkManager,NetworkConfig};
const TOPIC:&str="/quillon/datacenter/1/receipts";

#[derive(Clone)] struct App {
    config: Config, slots:Arc<Semaphore>, store:Arc<Mutex<Store>>,
    sigil:Arc<RwLock<Value>>, completed:Arc<std::sync::atomic::AtomicU64>,
    network:Arc<NetworkManager>, started:Instant,
}
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
struct Job { payload:String, #[serde(default="one")] rounds:u32 }
fn one()->u32 {1}
#[derive(Serialize)] struct Receipt {
    version:u8, input_hash:String, output_hash:String, rounds:u32,
    input_bytes:usize, elapsed_us:u128, execution:String, chain_settlement:String,
}
type ApiResult<T> = std::result::Result<Json<T>,(StatusCode,Json<Value>)>;
fn api_error(code:StatusCode,message:impl ToString)->(StatusCode,Json<Value>){(code,Json(json!({"error":message.to_string()})))}

/// Explicitly checks live service state and network topics, not a mere HTTP 200.
pub fn validate_sigil(v:&Value,prefix:&str)->Result<()> {
    let d=&v["data"];
    if v["ok"]!=true || d["started"]!=true || !d["peer_count"].is_u64()
        || !d["topics"].as_array().map(|ts|ts.iter().any(|t|t.as_str().map(|s|s.starts_with(prefix)).unwrap_or(false))).unwrap_or(false) {
        return Err("SIGIL topology is unavailable, stopped, or belongs to a different topic network".into());
    }
    Ok(())
async fn probe(config:&Config)->Result<Value> {
    let client=reqwest::Client::builder().timeout(Duration::from_secs(4)).redirect(reqwest::redirect::Policy::none()).build()?;
    let mut response=client.get(&config.sigil_topology_url).send().await?.error_for_status()?;
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await? {if bytes.len()+chunk.len()>262144{return Err("SIGIL response too large".into());} bytes.extend_from_slice(&chunk);}
    let v:Value=serde_json::from_slice(&bytes)?;validate_sigil(&v,&config.sigil_topic_prefix)?;
    Ok(json!({"reachable":true,"observation":"reported by existing SIGIL full node; not independent consensus verification",
        "network_topic_prefix":config.sigil_topic_prefix,"node":v["data"]["self_node_id"],"peers":v["data"]["peer_count"],
        "checked_unix_ms":now_ms(),"chain_settlement":false}))
}
fn now_ms()->u128{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()}
async fn status(State(a):State<App>)->Json<Value> {
    let sigil=a.sigil.read().await.clone();
    Json(json!({"name":a.config.name,"mode":"local CPU worker + Flux P2P + Aether + attached SIGIL full node",
        "uptime_s":a.started.elapsed().as_secs(),"workers":a.config.workers,"available_slots":a.slots.available_permits(),
        "completed_jobs":a.completed.load(std::sync::atomic::Ordering::Relaxed),
        "p2p_running":a.network.is_running(),"connected_peers":a.network.connected_peers().len(),
        "sigil":sigil,"aether":"local verified shards; no remote replication promise",
        "gpu_execution":false,"physical_datacenter_commissioned":false,
        "cortex_policy":if a.slots.available_permits()==0{"backpressure: reject excess work"}else{"admit within configured capacity"}}))
}
async fn ready(State(a):State<App>)->(StatusCode,Json<Value>){
    let v=a.sigil.read().await.clone();
    let fresh=v["checked_unix_ms"].as_u64().map(|t|now_ms().saturating_sub(t as u128)<20000).unwrap_or(false);
    let ok=a.network.is_running() && v["reachable"]==true && fresh;
    (if ok{StatusCode::OK}else{StatusCode::SERVICE_UNAVAILABLE},Json(json!({"ready":ok,"sigil":v})))
}
async fn job(State(a):State<App>,Json(j):Json<Job>)->ApiResult<Value>{
    if j.payload.len()>a.config.max_job_bytes || !(1..=10000).contains(&j.rounds){return Err(api_error(StatusCode::BAD_REQUEST,"payload or rounds exceeds configured limit"));}
    let permit=a.slots.clone().try_acquire_owned().map_err(|_|api_error(StatusCode::TOO_MANY_REQUESTS,"all CPU slots busy; retry later"))?;
    let store=a.store.clone();
    let value=tokio::task::spawn_blocking(move ||->Result<Value>{
        let _permit=permit;let start=Instant::now();let input=blake3::hash(j.payload.as_bytes());let mut out=input;
        for i in 1..j.rounds {let mut h=blake3::Hasher::new();h.update(out.as_bytes());h.update(&i.to_le_bytes());out=h.finalize();}
        let receipt=Receipt{version:1,input_hash:input.to_hex().to_string(),output_hash:out.to_hex().to_string(),rounds:j.rounds,
            input_bytes:j.payload.len(),elapsed_us:start.elapsed().as_micros(),execution:"local CPU BLAKE3 hash-chain diagnostic; not AI inference".into(),chain_settlement:"not submitted".into()};
        let bytes=serde_json::to_vec(&receipt)?;
        let id=store.lock().map_err(|_|"storage lock poisoned")?.put(&bytes)?;
        Ok(json!({"receipt":receipt,"object_id":id}))
    }).await.map_err(|e|api_error(StatusCode::INTERNAL_SERVER_ERROR,e))?.map_err(|e|api_error(StatusCode::INSUFFICIENT_STORAGE,e))?;
    a.completed.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
    // Advertise only object identity; never send prompts or storage keys to peers.
    let announcement=json!({"version":1,"kind":"receipt_available","object_id":value["object_id"]});
    let queued=a.network.publish(TOPIC,serde_json::to_vec(&announcement).unwrap()).is_ok();
    Ok(Json(json!({"result":value,"gossip_queued":queued,"gossip_delivery_verified":false})))
}
async fn object(State(a):State<App>,Path(id):Path<String>)->ApiResult<Value>{
    let bytes=a.store.lock().map_err(|_|api_error(StatusCode::INTERNAL_SERVER_ERROR,"storage lock"))?.get(&id)
        .map_err(|e|api_error(StatusCode::NOT_FOUND,e))?;
    Ok(Json(serde_json::from_slice(&bytes).map_err(|e|api_error(StatusCode::INTERNAL_SERVER_ERROR,e))?))
}

pub fn audit(config:&Config)->Result<Value>{
    let root=fs::canonicalize(&config.workspace)?;
    let index=flux_refactor::api_index::build_index(root.to_str().ok_or("workspace must be UTF-8")?);
    let report=flux_refactor::mismatch::audit_crate(root.join("crates/flux-skyskraber").to_str().unwrap(),&index);
    let ws=flux_graph::resolve_workspace(&root)?;
    let mut cortex=flux_cortex::Cortex::new(ws);
    let advisory=cortex.run_loop(flux_optimize::OptimizationPreset::Balanced);
    Ok(json!({"refactor":report,"cortex_advisory":advisory,
        "classification":"heuristic source advisory only; upstream Cortex validation is simulated, not measured speedup",
        "automatic_source_rewrite":false,"automatic_consensus_change":false}))
}

pub async fn run(config:Config,run_seconds:Option<u64>)->Result<()> {
    config.validate()?;
    fs::create_dir_all(&config.data_dir)?;
    // OS advisory lock is released on crash, unlike a stale PID file.
    let lock=fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(config.data_dir.join("node.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_|"another node owns this data directory")?;
    let store=Store::open(&config.data_dir.join("aether"),config.storage_quota_bytes)?;
    let identity_path=config.data_dir.join("p2p.identity");
    if !identity_path.exists(){use rand::RngCore;let mut b=[0u8;32];rand::rngs::OsRng.fill_bytes(&mut b);private_file(&identity_path,hex::encode(b).as_bytes())?;}
    let identity=fs::read_to_string(&identity_path)?;
    if identity.len()!=64 || !identity.bytes().all(|c|c.is_ascii_hexdigit()){return Err("invalid P2P identity file".into());}
    // flux-p2p derives its key from node_id: use a persisted random secret, never a public name.
    let initial=probe(&config).await?;
    let listener=tokio::net::TcpListener::bind(config.http_listen).await?;
    let net_config=NetworkConfig{node_id:identity,listen_addr:format!("/ip4/127.0.0.1/tcp/{}",config.p2p_port),
        bootstrap_peers:config.bootstrap_peers.clone(),dagknight_enabled:false,sap_enabled:true,x_algo_enabled:true,
        entanglement_enabled:false,gossipsub_topics:vec![TOPIC.into()]};
    let mut net=NetworkManager::new(net_config);net.start().await.map_err(|e|format!("P2P: {e}"))?;
    let deadline=Instant::now()+Duration::from_secs(5);let mut listening=false;
    while Instant::now()<deadline {
        // Confirm this swarm's listener event, not another process on the port.
        if net.drain_events().iter().any(|e|matches!(e,flux_p2p::swarm::SwarmAppEvent::NewListenAddr(_))){listening=true;break;}
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if !listening || !net.is_running(){let _=net.stop().await;return Err("P2P did not become ready".into());}
    let net=Arc::new(net);
    let app=App{slots:Arc::new(Semaphore::new(config.workers)),store:Arc::new(Mutex::new(store)),
        sigil:Arc::new(RwLock::new(initial)),completed:Arc::new(std::sync::atomic::AtomicU64::new(0)),network:net.clone(),started:Instant::now(),config:config.clone()};
    let watcher=app.clone();let monitoring=tokio::spawn(async move {
        loop {tokio::time::sleep(Duration::from_secs(5)).await;
            let v=match probe(&watcher.config).await{Ok(v)=>v,Err(e)=>json!({"reachable":false,"error":e.to_string(),"checked_unix_ms":now_ms()})};
            *watcher.sigil.write().await=v;
            // Drain untrusted network events without executing their contents.
            watcher.network.drain_events();
        }
    });
    let routes=Router::new().route("/status",get(status)).route("/ready",get(ready))
        .route("/jobs/hash",post(job)).route("/objects/:id",get(object))
        .layer(DefaultBodyLimit::max(config.max_job_bytes*6+1024)).with_state(app);
    println!("Quillon datacenter node ready: http://{} (SIGIL attached, CPU workers {}, P2P port {})",config.http_listen,config.workers,config.p2p_port);
    let result=axum::serve(listener,routes).with_graceful_shutdown(async move {
        #[cfg(unix)] {
            let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
            tokio::select!{_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{},_=async{if let Some(s)=run_seconds{tokio::time::sleep(Duration::from_secs(s)).await}else{std::future::pending::<()>().await}}=>{}}
        }
        #[cfg(not(unix))] {tokio::select!{_=tokio::signal::ctrl_c()=>{},_=async{if let Some(s)=run_seconds{tokio::time::sleep(Duration::from_secs(s)).await}else{std::future::pending::<()>().await}}=>{}}}
    }).await;
    monitoring.abort();let _=net.stop().await;drop(lock);result?;Ok(())
}
#[cfg(test)] mod tests {use super::*;
    #[test] fn sigil_requires_correct_network_and_running_state(){
        let mut v=json!({"ok":true,"data":{"started":true,"peer_count":0,"topics":["/sigil/g2/blocks"]}});
        validate_sigil(&v,"/sigil/g2/").unwrap();assert!(validate_sigil(&v,"/sigil/g0/").is_err());
        v["data"]["started"]=json!(false);assert!(validate_sigil(&v,"/sigil/g2/").is_err());
    }
}
