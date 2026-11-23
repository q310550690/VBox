/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.webkit;

/** 安卓回调接口（WebView/CookieManager 用）。 */
public interface ValueCallback<T> {
    void onReceiveValue(T value);
}
