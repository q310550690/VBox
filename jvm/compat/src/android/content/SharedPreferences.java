/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.content;

import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/**
 * SharedPreferences 兼容层。
 *
 * 桌面端没有 XML 偏好文件；`Mem` 是纯内存实现（编程式设置 + 读取即得）。
 * 需要「预置凭据」时由宿主把种子写入 JVM 侧的 prefs 影子实现（P4 的 prefs-shim），
 * 这里不做磁盘 IO —— 共享内存状态在单个宿主进程内即足够。
 */
public interface SharedPreferences {

    Map<String, ?> getAll();

    String getString(String key, String defValue);

    Set<String> getStringSet(String key, Set<String> defValues);

    int getInt(String key, int defValue);

    long getLong(String key, long defValue);

    float getFloat(String key, float defValue);

    boolean getBoolean(String key, boolean defValue);

    boolean contains(String key);

    Editor edit();

    void registerOnSharedPreferenceChangeListener(OnSharedPreferenceChangeListener listener);

    void unregisterOnSharedPreferenceChangeListener(OnSharedPreferenceChangeListener listener);

    interface Editor {
        Editor putString(String key, String value);
        Editor putStringSet(String key, Set<String> values);
        Editor putInt(String key, int value);
        Editor putLong(String key, long value);
        Editor putFloat(String key, float value);
        Editor putBoolean(String key, boolean value);
        Editor remove(String key);
        Editor clear();
        boolean commit();
        void apply();
    }

    interface OnSharedPreferenceChangeListener {
        void onSharedPreferenceChanged(SharedPreferences sharedPreferences, String key);
    }

    /** 纯内存实现（每个 name 一个实例）。 */
    final class Mem implements SharedPreferences {

        private static final ConcurrentHashMap<String, Mem> INSTANCES = new ConcurrentHashMap<String, Mem>();

        private final ConcurrentHashMap<String, Object> values = new ConcurrentHashMap<String, Object>();
        private final Set<OnSharedPreferenceChangeListener> listeners =
                java.util.Collections.newSetFromMap(new ConcurrentHashMap<OnSharedPreferenceChangeListener, Boolean>());

        private Mem() { }

        public static SharedPreferences get(String name) {
            String key = name == null || name.isEmpty() ? "default" : name;
            Mem created = new Mem();
            Mem existing = INSTANCES.putIfAbsent(key, created);
            return existing == null ? created : existing;
        }

        @Override
        public Map<String, ?> getAll() {
            return new HashMap<String, Object>(values);
        }

        @Override
        public String getString(String key, String defValue) {
            Object v = values.get(key);
            return v instanceof String ? (String) v : defValue;
        }

        @Override
        @SuppressWarnings("unchecked")
        public Set<String> getStringSet(String key, Set<String> defValues) {
            Object v = values.get(key);
            return v instanceof Set ? new HashSet<String>((Set<String>) v) : defValues;
        }

        @Override
        public int getInt(String key, int defValue) {
            Object v = values.get(key);
            return v instanceof Number ? ((Number) v).intValue() : defValue;
        }

        @Override
        public long getLong(String key, long defValue) {
            Object v = values.get(key);
            return v instanceof Number ? ((Number) v).longValue() : defValue;
        }

        @Override
        public float getFloat(String key, float defValue) {
            Object v = values.get(key);
            return v instanceof Number ? ((Number) v).floatValue() : defValue;
        }

        @Override
        public boolean getBoolean(String key, boolean defValue) {
            Object v = values.get(key);
            return v instanceof Boolean ? (Boolean) v : defValue;
        }

        @Override
        public boolean contains(String key) {
            return values.containsKey(key);
        }

        @Override
        public Editor edit() {
            return new MemEditor();
        }

        @Override
        public void registerOnSharedPreferenceChangeListener(OnSharedPreferenceChangeListener listener) {
            if (listener != null) listeners.add(listener);
        }

        @Override
        public void unregisterOnSharedPreferenceChangeListener(OnSharedPreferenceChangeListener listener) {
            if (listener != null) listeners.remove(listener);
        }

        private void notifyChanged(String key) {
            for (OnSharedPreferenceChangeListener l : listeners) {
                try {
                    l.onSharedPreferenceChanged(this, key);
                } catch (Throwable ignored) {
                    // 监听器异常不能污染写入方
                }
            }
        }

        private final class MemEditor implements Editor {

            private final Map<String, Object> staged = new HashMap<String, Object>();
            private final Set<String> removed = new HashSet<String>();
            private boolean cleared;

            @Override
            public Editor putString(String key, String value) {
                return stage(key, value);
            }

            @Override
            public Editor putStringSet(String key, Set<String> values) {
                return stage(key, values == null ? null : new HashSet<String>(values));
            }

            @Override
            public Editor putInt(String key, int value) {
                return stage(key, value);
            }

            @Override
            public Editor putLong(String key, long value) {
                return stage(key, value);
            }

            @Override
            public Editor putFloat(String key, float value) {
                return stage(key, value);
            }

            @Override
            public Editor putBoolean(String key, boolean value) {
                return stage(key, value);
            }

            private Editor stage(String key, Object value) {
                if (key != null) staged.put(key, value);
                return this;
            }

            @Override
            public Editor remove(String key) {
                if (key != null) removed.add(key);
                return this;
            }

            @Override
            public Editor clear() {
                cleared = true;
                return this;
            }

            @Override
            public boolean commit() {
                apply();
                return true;
            }

            @Override
            public void apply() {
                if (cleared) values.clear();
                for (String key : removed) values.remove(key);
                for (Map.Entry<String, Object> e : staged.entrySet()) {
                    if (e.getValue() == null) values.remove(e.getKey());
                    else values.put(e.getKey(), e.getValue());
                }
                for (String key : staged.keySet()) notifyChanged(key);
                staged.clear();
                removed.clear();
                cleared = false;
            }
        }
    }
}
