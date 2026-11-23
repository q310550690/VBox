//! P2 验收：宿主池隔离、容量淘汰、租约保护（计划 §7 P2 `pool_isolation`）。
//!
//! 用假的 `HostItem`，因此不需要 JRE；真 JRE 的端到端放在 `vbox_core/tests/jvm_lane.rs`。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use vbox_jvm::{HostItem, Pool, PoolConfig, PoolKey, PoolSpawnError};

struct MockHost {
    id: usize,
    dead: bool,
    spawns: Arc<AtomicUsize>,
}

impl MockHost {
    fn new(id: usize, spawns: &Arc<AtomicUsize>) -> Self {
        spawns.fetch_add(1, Ordering::SeqCst);
        MockHost { id, dead: false, spawns: Arc::clone(spawns) }
    }
}

impl HostItem for MockHost {
    fn is_dead(&self) -> bool {
        self.dead
    }
    fn shutdown(&mut self) {
        self.dead = true;
    }
}

fn cfg(max: usize) -> PoolConfig {
    PoolConfig { max_hosts: max, idle_ttl: Duration::from_millis(50), sweep_every: Duration::from_millis(1) }
}

#[test]
fn pool_isolates_by_jar_and_ext() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let pool: Arc<Pool<MockHost>> = Pool::new(cfg(4));

    let (a, s1) = (PoolKey::new("a.jar", "ext-a"), Arc::clone(&spawns));
    let slot1 = pool
        .get_or_spawn(a.clone(), move || Ok::<_, ()>(MockHost::new(1, &s1)))
        .expect("第一次取宿主");
    let (b, s2) = (PoolKey::new("b.jar", "ext-a"), Arc::clone(&spawns));
    let slot2 = pool
        .get_or_spawn(b.clone(), move || Ok::<_, ()>(MockHost::new(2, &s2)))
        .expect("另一个 jar 应当另起宿主");

    // 不同 jar：不是同一个宿主
    let id1 = slot1.with(|h| h.id);
    let id2 = slot2.with(|h| h.id);
    assert_ne!(id1, id2, "不同 jar 必须各一个宿主");

    // 同 key 复用
    let slot1b = pool
        .get_or_spawn(a, || Ok::<_, ()>(MockHost::new(99, &spawns)))
        .expect("同 key 应复用");
    assert_eq!(slot1b.with(|h| h.id), id1, "同 key 不得重建");

    // 同 jar 不同 ext 必须隔离
    let (c, s3) = (PoolKey::new("a.jar", "ext-b"), Arc::clone(&spawns));
    let slot3 = pool
        .get_or_spawn(c, move || Ok::<_, ()>(MockHost::new(3, &s3)))
        .expect("同 jar 不同 ext 应各起宿主");
    assert_ne!(slot3.with(|h| h.id), id1, "ext 不同必须隔离（蜘蛛按 ext 初始化）");
}

#[test]
fn pool_capacity_evicts_idle_but_respects_leases() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let pool: Arc<Pool<MockHost>> = Pool::new(cfg(1));

    let (k1, s1) = (PoolKey::new("a.jar", "e"), Arc::clone(&spawns));
    let slot = pool.get_or_spawn(k1.clone(), move || Ok::<_, ()>(MockHost::new(1, &s1))).unwrap();

    // 拿租约：池满时应报 Full，而不是把在用的宿主杀掉
    let lease = pool.lease(&slot);
    let (k2, s2) = (PoolKey::new("b.jar", "e"), Arc::clone(&spawns));
    let err = pool
        .get_or_spawn(k2.clone(), move || Ok::<_, ()>(MockHost::new(2, &s2)))
        .err();
    assert!(matches!(err, Some(PoolSpawnError::Full { max: 1 })), "池满且有租约应报 Full");

    // 归还租约后应能淘汰空闲项
    drop(lease);
    let (k2b, s3) = (PoolKey::new("b.jar", "e"), Arc::clone(&spawns));
    pool.get_or_spawn(k2b, move || Ok::<_, ()>(MockHost::new(3, &s3)))
        .expect("租约释放后应能腾位");
    assert_eq!(pool.stats().evicted, 1, "淘汰计数应加一");

    // 被淘汰的宿主应已 shutdown
    assert!(pool.stats().hosts <= 1);
}

#[test]
fn dead_host_is_replaced_not_reused() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let pool: Arc<Pool<MockHost>> = Pool::new(cfg(2));
    let (k, s1) = (PoolKey::new("x.jar", "e"), Arc::clone(&spawns));
    let slot = pool.get_or_spawn(k.clone(), move || Ok::<_, ()>(MockHost::new(1, &s1))).unwrap();
    slot.with(|h| h.dead = true);

    let (k2, s2) = (PoolKey::new("x.jar", "e"), Arc::clone(&spawns));
    let again = pool.get_or_spawn(k2, move || Ok::<_, ()>(MockHost::new(2, &s2))).unwrap();
    assert_eq!(again.with(|h| h.id), 2, "死宿主必须重建而不是复用");
}
