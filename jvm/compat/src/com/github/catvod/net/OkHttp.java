/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package com.github.catvod.net;

import java.io.IOException;
import java.util.Map;
import java.util.concurrent.TimeUnit;

import okhttp3.Dns;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;

/**
 * com.github.catvod.net.OkHttp 的桌面实现。
 *
 * 真实 TVBox 里这是带缓存/并发控制/UA 注入的工具类；桌面端只需要「能发请求」，
 * 所以直接复用 okhttp3（兼容层已随包带 3.14.9）并做最小封装。
 */
public final class OkHttp {

    private static volatile OkHttpClient CLIENT;

    private OkHttp() { }

    public static OkHttpClient client() {
        OkHttpClient c = CLIENT;
        if (c == null) {
            synchronized (OkHttp.class) {
                c = CLIENT;
                if (c == null) {
                    c = new OkHttpClient.Builder()
                            .connectTimeout(15, TimeUnit.SECONDS)
                            .readTimeout(20, TimeUnit.SECONDS)
                            .writeTimeout(20, TimeUnit.SECONDS)
                            .followRedirects(true)
                            .followSslRedirects(true)
                            .build();
                    CLIENT = c;
                }
            }
        }
        return c;
    }

    /** 与安卓侧一致的 DNS（系统解析）；保留参数以便蜘蛛反射调用。 */
    public static Dns dns() {
        return Dns.SYSTEM;
    }

    public static void reset() {
        CLIENT = null;
    }

    public static String string(String url) {
        return string(url, null);
    }

    public static String string(String url, Map<String, String> headers) {
        return string(url, headers, null);
    }

    public static String string(String url, Map<String, String> headers, Map<String, String> params) {
        if (url == null || url.isEmpty()) return "";
        String full = url;
        if (params != null && !params.isEmpty()) {
            StringBuilder sb = new StringBuilder(url);
            char sep = url.indexOf('?') >= 0 ? '&' : '?';
            for (Map.Entry<String, String> e : params.entrySet()) {
                sb.append(sep).append(e.getKey()).append('=').append(e.getValue());
                sep = '&';
            }
            full = sb.toString();
        }
        Request.Builder builder = new Request.Builder().url(full).get();
        if (headers != null) {
            for (Map.Entry<String, String> e : headers.entrySet()) {
                if (e.getKey() != null && e.getValue() != null) builder.header(e.getKey(), e.getValue());
            }
        }
        return execute(builder.build());
    }

    public static String post(String url, String body, Map<String, String> headers) {
        if (url == null || url.isEmpty()) return "";
        okhttp3.RequestBody rb = okhttp3.RequestBody.create(okhttp3.MediaType.parse("application/json;charset=utf-8"),
                body == null ? "" : body);
        Request.Builder builder = new Request.Builder().url(url).post(rb);
        if (headers != null) {
            for (Map.Entry<String, String> e : headers.entrySet()) {
                if (e.getKey() != null && e.getValue() != null) builder.header(e.getKey(), e.getValue());
            }
        }
        return execute(builder.build());
    }

    public static Response newCall(Request request) throws IOException {
        return client().newCall(request).execute();
    }

    public static String execute(Request request) {
        try (Response response = client().newCall(request).execute()) {
            return response.body() == null ? "" : response.body().string();
        } catch (Throwable t) {
            android.util.Log.w("OkHttp", request.url() + " 失败: " + t);
            return "";
        }
    }
}
