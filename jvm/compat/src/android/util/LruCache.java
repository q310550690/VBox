// VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）
package android.util;

import java.util.LinkedHashMap;
import java.util.Map;

/** 最小 LRU 缓存：蜘蛛常用来缓存分类/详情响应。 */
public class LruCache<K, V> {
  private final LinkedHashMap<K, V> map;
  private int size;
  private int maxSize;

  public LruCache(int maxSize) {
    if (maxSize <= 0) {
      throw new IllegalArgumentException("maxSize <= 0");
    }
    this.maxSize = maxSize;
    this.map = new LinkedHashMap<K, V>(0, 0.75f, true);
  }

  public final V get(K key) {
    if (key == null) throw new NullPointerException("key == null");
    return map.get(key);
  }

  public final V put(K key, V value) {
    if (key == null || value == null) throw new NullPointerException("key == null || value == null");
    V previous = map.put(key, value);
    size += 1;
    trimToSize(maxSize);
    return previous;
  }

  public final V remove(K key) {
    if (key == null) throw new NullPointerException("key == null");
    V previous = map.remove(key);
    if (previous != null) size -= 1;
    return previous;
  }

  public final int size() { return size; }
  public final int maxSize() { return maxSize; }
  public final int hitCount() { return 0; }
  public final int missCount() { return 0; }

  public void evictAll() {
    trimToSize(-1);
  }

  public final synchronized Map<K, V> snapshot() {
    return new LinkedHashMap<K, V>(map);
  }

  private void trimToSize(int max) {
    while (true) {
      if (size <= max) return;
      if (map.isEmpty()) return;
      Map.Entry<K, V> eldest = map.entrySet().iterator().next();
      K key = eldest.getKey();
      V value = eldest.getValue();
      map.remove(key);
      size -= 1;
      entryRemoved(true, key, value, null);
    }
  }

  protected void entryRemoved(boolean evicted, K key, V oldValue, V newValue) {}
}
