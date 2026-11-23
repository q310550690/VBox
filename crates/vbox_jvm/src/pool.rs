//! 宿主进程池（P2）：有界、按 `(jar 标识, ext 哈希)` 分片、空闲回收、播放租约。
//!
//! 设计约束（计划 §7 P2）：
//! - **有界**：同时存活的宿主不超过 `max_hosts`，满了先淘汰最久未用且没有租约的；
//! - **不打断播放**：`player_content` 拿到的是长期 URL（蜘蛛内部代理端口），
//!   播放期间必须给宿主加租约，回收线程跳过 `leases > 0` 的条目；
//! - **可观测**：`stats()` 暴露 spawn/hit/reap/evict/calls/failures，供诊断面板与 CLI 用；
//! - **可测试**：用 `HostItem` 抽象宿主，单测不需要真 JRE。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// 池里的宿主抽象：只需要能判断死活、能被关掉。
pub trait HostItem: Send + 'static {
    fn is_dead(&self) -> bool;
    fn shutdown(&mut self);
}

/// 池键：jar 的稳定标识（md5/sha256 前缀）+ ext 的哈希。
///
/// 同一个 jar 但 ext 不同 ⇒ 蜘蛛初始化参数不同（网盘 cookie、站点配置），
/// 必须各自一个宿主，不能复用。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub jar: String,
    pub ext_hash: u64,
}

impl PoolKey {
    pub fn new(jar: impl Into<String>, ext: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        ext.hash(&mut hasher);
        PoolKey {
            jar: jar.into(),
            ext_hash: hasher.finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// 同时存活的宿主上限。
    pub max_hosts: usize,
    /// 空闲多久后可回收。
    pub idle_ttl: Duration,
    /// 惰性清扫的最小间隔（避免每次 acquire 都全表扫）。
    pub sweep_every: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            max_hosts: 8,
            idle_ttl: Duration::from_secs(600),
            sweep_every: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// 当前存活宿主数。
    pub hosts: usize,
    pub max_hosts: usize,
    /// 累计 spawn 次数。
    pub spawns: u64,
    /// 命中已有宿主次数。
    pub hits: u64,
    /// 空闲回收数量。
    pub reaped: u64,
    /// 容量淘汰数量。
    pub evicted: u64,
    pub calls: u64,
    pub failures: u64,
}

/// 池层面的失败：容量满，或调用方 spawn 失败（保留原始错误类型 `E`）。
#[derive(Debug)]
pub enum PoolSpawnError<E> {
    /// 池满且没有可淘汰的条目（全部都有租约）。
    Full { max: usize },
    /// 新建宿主失败（由调用方定义错误类型）。
    Spawn(E),
}

impl<E: std::fmt::Debug> std::fmt::Display for PoolSpawnError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoolSpawnError::Full { max } => write!(f, "JVM 宿主池已满（{} 个都在使用中）", max),
            PoolSpawnError::Spawn(e) => write!(f, "新建 JVM 宿主失败：{e:?}"),
        }
    }
}

impl<E: std::fmt::Debug> std::error::Error for PoolSpawnError<E> {}

struct SlotMeta {
    last_used: Instant,
    created: Instant,
    leases: u32,
}

pub struct Slot<T: HostItem> {
    pub key: PoolKey,
    item: Mutex<T>,
    meta: Mutex<SlotMeta>,
}

impl<T: HostItem> Slot<T> {
    pub fn key(&self) -> &PoolKey {
        &self.key
    }

    /// 直接操作宿主（调用方自己保证串行）。
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut guard = self.item.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    pub fn touch(&self) {
        let mut meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        meta.last_used = Instant::now();
    }

    fn leases(&self) -> u32 {
        self.meta.lock().unwrap_or_else(|e| e.into_inner()).leases
    }

    fn add_lease(&self) {
        let mut meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        meta.leases = meta.leases.saturating_add(1);
    }

    fn release_lease(&self) {
        let mut meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        meta.leases = meta.leases.saturating_sub(1);
    }

    fn last_used(&self) -> Instant {
        self.meta.lock().unwrap_or_else(|e| e.into_inner()).last_used
    }

    pub fn created(&self) -> Instant {
        self.meta.lock().unwrap_or_else(|e| e.into_inner()).created
    }
}

/// 播放租约：存活期间对应宿主不会被空闲回收。
pub struct Lease<T: HostItem> {
    slot: Arc<Slot<T>>,
    pool: Weak<Pool<T>>,
}

impl<T: HostItem> Drop for Lease<T> {
    fn drop(&mut self) {
        self.slot.release_lease();
        if let Some(pool) = self.pool.upgrade() {
            let mut inner = pool.lock();
            inner.leases = inner.leases.saturating_sub(1);
        }
    }
}

struct PoolCounters {
    spawns: AtomicU64,
    hits: AtomicU64,
    reaped: AtomicU64,
    evicted: AtomicU64,
    calls: AtomicU64,
    failures: AtomicU64,
}

impl Default for PoolCounters {
    fn default() -> Self {
        Self {
            spawns: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            reaped: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        }
    }
}

struct Inner<T: HostItem> {
    entries: HashMap<PoolKey, Arc<Slot<T>>>,
    last_sweep: Instant,
    /// 全局租约数（快速判断「有播放在进行」）。
    leases: u32,
}

/// 有界宿主池。用 `Arc<Pool<T>>` 共享；池自身不 spawn 线程，回收是**惰性**的
/// （acquire/sweep 时按 `sweep_every` 节流触发），便于测试与单实例控制。
pub struct Pool<T: HostItem> {
    cfg: PoolConfig,
    inner: Mutex<Inner<T>>,
    counters: PoolCounters,
}

impl<T: HostItem> Pool<T> {
    pub fn new(cfg: PoolConfig) -> Arc<Self> {
        Arc::new(Pool {
            cfg,
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                last_sweep: Instant::now(),
                leases: 0,
            }),
            counters: PoolCounters::default(),
        })
    }

    pub fn config(&self) -> &PoolConfig {
        &self.cfg
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 取或建一个宿主。`spawn` 只在需要新建时调用（慢操作，放在锁外）。
    pub fn get_or_spawn<E, F>(
        self: &Arc<Self>,
        key: PoolKey,
        spawn: F,
    ) -> Result<Arc<Slot<T>>, PoolSpawnError<E>>
    where
        F: FnOnce() -> Result<T, E>,
    {
        // 快路径：已有且活着
        {
            let mut inner = self.lock();
            self.maybe_sweep_locked(&mut inner);
            let existing = inner.entries.get(&key).cloned();
            if let Some(slot) = existing {
                if !slot.with(|item| item.is_dead()) {
                    slot.touch();
                    self.counters.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(slot);
                }
                // 死宿主：摘掉，走重建
                inner.entries.remove(&key);
            }
            self.make_room_locked(&mut inner)?;
        }

        let item = spawn().map_err(PoolSpawnError::Spawn)?;
        let now = Instant::now();
        let fresh = Arc::new(Slot {
            key: key.clone(),
            item: Mutex::new(item),
            meta: Mutex::new(SlotMeta {
                last_used: now,
                created: now,
                leases: 0,
            }),
        });

        let mut inner = self.lock();
        // 双检：并发下别人可能刚建好
        let raced = inner.entries.get(&key).cloned();
        if let Some(existing) = raced {
            if !existing.with(|item| item.is_dead()) {
                existing.touch();
                self.counters.hits.fetch_add(1, Ordering::Relaxed);
                drop(inner);
                // 我们这份多余，关掉
                fresh.with(|item| item.shutdown());
                return Ok(existing);
            }
            inner.entries.remove(&key);
        }
        self.make_room_locked(&mut inner)?;
        inner.entries.insert(key, Arc::clone(&fresh));
        self.counters.spawns.fetch_add(1, Ordering::Relaxed);
        Ok(fresh)
    }

    pub fn lease(self: &Arc<Self>, slot: &Arc<Slot<T>>) -> Lease<T> {
        slot.add_lease();
        {
            let mut inner = self.lock();
            inner.leases = inner.leases.saturating_add(1);
        }
        Lease {
            slot: Arc::clone(slot),
            pool: Arc::downgrade(self),
        }
    }

    fn make_room_locked<E>(&self, inner: &mut Inner<T>) -> Result<(), PoolSpawnError<E>> {
        if inner.entries.len() < self.cfg.max_hosts {
            return Ok(());
        }
        // 淘汰最久未用且无租约的
        let victim = inner
            .entries
            .iter()
            .filter(|(_, slot)| slot.leases() == 0)
            .min_by_key(|(_, slot)| slot.last_used())
            .map(|(key, _)| key.clone());
        match victim {
            Some(key) => {
                if let Some(slot) = inner.entries.remove(&key) {
                    slot.with(|item| item.shutdown());
                    self.counters.evicted.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            }
            None => Err(PoolSpawnError::Full {
                max: self.cfg.max_hosts,
            }),
        }
    }

    fn maybe_sweep_locked(&self, inner: &mut Inner<T>) {
        if inner.last_sweep.elapsed() < self.cfg.sweep_every {
            return;
        }
        inner.last_sweep = Instant::now();
        self.reap_locked(inner);
    }

    fn reap_locked(&self, inner: &mut Inner<T>) -> usize {
        if inner.leases > 0 {
            // 有播放租约时保守：只回收明显空闲的
        }
        let now = Instant::now();
        let ttl = self.cfg.idle_ttl;
        let stale: Vec<PoolKey> = inner
            .entries
            .iter()
            .filter(|(_, slot)| slot.leases() == 0 && now.duration_since(slot.last_used()) >= ttl)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &stale {
            if let Some(slot) = inner.entries.remove(key) {
                slot.with(|item| item.shutdown());
                self.counters.reaped.fetch_add(1, Ordering::Relaxed);
            }
        }
        stale.len()
    }

    /// 显式清扫一次（诊断/退出前调用），返回回收数量。
    pub fn sweep(&self) -> usize {
        let mut inner = self.lock();
        self.reap_locked(&mut inner)
    }

    /// 记录一次调用结果（成功/失败都记）。
    pub fn record_call(&self, ok: bool) {
        self.counters.calls.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.counters.failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn stats(&self) -> PoolStats {
        let inner = self.lock();
        PoolStats {
            hosts: inner.entries.len(),
            max_hosts: self.cfg.max_hosts,
            spawns: self.counters.spawns.load(Ordering::Relaxed),
            hits: self.counters.hits.load(Ordering::Relaxed),
            reaped: self.counters.reaped.load(Ordering::Relaxed),
            evicted: self.counters.evicted.load(Ordering::Relaxed),
            calls: self.counters.calls.load(Ordering::Relaxed),
            failures: self.counters.failures.load(Ordering::Relaxed),
        }
    }

    /// 把某个 key 的宿主从池里摘掉（崩溃重建前调用）。返回是否真的摘掉了。
    pub fn evict(&self, key: &PoolKey) -> bool {
        let mut inner = self.lock();
        let removed = inner.entries.remove(key).is_some();
        if removed {
            self.counters.evicted.fetch_add(1, Ordering::Relaxed);
        }
        removed
    }

    /// 关掉所有宿主（应用退出）。
    pub fn shutdown_all(&self) {
        let mut inner = self.lock();
        let slots: Vec<Arc<Slot<T>>> = inner.entries.drain().map(|(_, v)| v).collect();
        drop(inner);
        for slot in slots {
            slot.with(|item| item.shutdown());
        }
    }

    #[cfg(test)]
    fn contains(&self, key: &PoolKey) -> bool {
        self.lock().entries.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;

    struct MockHost {
        #[allow(dead_code)]
        id: usize,
        dead: Arc<AtomicBool>,
        shutdowns: Arc<AtomicUsize>,
    }

    impl HostItem for MockHost {
        fn is_dead(&self) -> bool {
            self.dead.load(Ordering::Relaxed)
        }
        fn shutdown(&mut self) {
            self.dead.store(true, Ordering::Relaxed);
            self.shutdowns.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn spawner(id: usize, shutdowns: &Arc<AtomicUsize>) -> impl Fn() -> Result<MockHost, ()> + '_ {
        let dead = Arc::new(AtomicBool::new(false));
        let shutdowns = Arc::clone(shutdowns);
        let _ = id;
        move || {
            Ok(MockHost {
                id,
                dead: Arc::clone(&dead),
                shutdowns: Arc::clone(&shutdowns),
            })
        }
    }

    #[test]
    fn reuses_same_key_and_isolates_different_jar() {
        let pool: Arc<Pool<MockHost>> = Pool::new(PoolConfig::default());
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let k1 = PoolKey::new("jar-a", "ext-1");
        let k2 = PoolKey::new("jar-b", "ext-1");

        let a1 = pool.get_or_spawn(k1.clone(), spawner(1, &shutdowns)).unwrap();
        let a2 = pool.get_or_spawn(k1.clone(), spawner(2, &shutdowns)).unwrap();
        assert!(Arc::ptr_eq(&a1, &a2), "同 key 必须复用同一个宿主");
        assert_eq!(pool.stats().spawns, 1);
        assert_eq!(pool.stats().hits, 1);

        let b = pool.get_or_spawn(k2.clone(), spawner(3, &shutdowns)).unwrap();
        assert!(!Arc::ptr_eq(&a1, &b), "不同 jar 必须隔离");
        assert_eq!(pool.stats().hosts, 2);
        assert_eq!(shutdowns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn ext_difference_splits_hosts() {
        let pool: Arc<Pool<MockHost>> = Pool::new(PoolConfig::default());
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let a = pool
            .get_or_spawn(PoolKey::new("jar-a", "{\"cookie\":1}"), spawner(1, &shutdowns))
            .unwrap();
        let b = pool
            .get_or_spawn(PoolKey::new("jar-a", "{\"cookie\":2}"), spawner(2, &shutdowns))
            .unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(pool.stats().spawns, 2);
    }

    #[test]
    fn dead_host_is_replaced() {
        let pool: Arc<Pool<MockHost>> = Pool::new(PoolConfig::default());
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let key = PoolKey::new("jar-a", "");
        let first = pool.get_or_spawn(key.clone(), spawner(1, &shutdowns)).unwrap();
        first.with(|h| h.shutdown());
        let second = pool.get_or_spawn(key.clone(), spawner(2, &shutdowns)).unwrap();
        assert_eq!(pool.stats().spawns, 2);
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn lease_blocks_reaping() {
        let pool: Arc<Pool<MockHost>> = Pool::new(PoolConfig {
            max_hosts: 4,
            idle_ttl: Duration::from_millis(0),
            sweep_every: Duration::from_millis(0),
        });
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let slot = pool
            .get_or_spawn(PoolKey::new("jar-a", ""), spawner(1, &shutdowns))
            .unwrap();
        let lease = pool.lease(&slot);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(pool.sweep(), 0, "有租约的宿主不能被回收");
        drop(lease);
        assert_eq!(pool.sweep(), 1, "租约释放后可以回收");
        assert_eq!(pool.stats().hosts, 0);
        assert_eq!(shutdowns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn capacity_evicts_idle_lru() {
        let pool: Arc<Pool<MockHost>> = Pool::new(PoolConfig {
            max_hosts: 2,
            idle_ttl: Duration::from_secs(600),
            sweep_every: Duration::from_secs(600),
        });
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let a = pool.get_or_spawn(PoolKey::new("a", ""), spawner(1, &shutdowns)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let b = pool.get_or_spawn(PoolKey::new("b", ""), spawner(2, &shutdowns)).unwrap();
        a.touch();
        std::thread::sleep(Duration::from_millis(2));
        // 需要第三个，容量 2：应淘汰 b（a 刚 touch 过）
        let c = pool.get_or_spawn(PoolKey::new("c", ""), spawner(3, &shutdowns)).unwrap();
        assert_eq!(pool.stats().hosts, 2);
        assert_eq!(pool.stats().evicted, 1);
        assert!(pool.contains(&PoolKey::new("a", "")));
        assert!(!pool.contains(&PoolKey::new("b", "")));
        assert!(pool.contains(&PoolKey::new("c", "")));
        let _ = (b, c);
    }

    #[test]
    fn full_pool_with_leases_fails() {
        let pool: Arc<Pool<MockHost>> = Pool::new(PoolConfig {
            max_hosts: 1,
            idle_ttl: Duration::from_secs(600),
            sweep_every: Duration::from_secs(600),
        });
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let only = pool.get_or_spawn(PoolKey::new("a", ""), spawner(1, &shutdowns)).unwrap();
        let _lease = pool.lease(&only);
        let err = pool
            .get_or_spawn(PoolKey::new("b", ""), spawner(2, &shutdowns))
            .err()
            .expect("池已满且全部被租约占住，应当报错");
        assert!(matches!(err, PoolSpawnError::Full { max: 1 }));
    }
}
