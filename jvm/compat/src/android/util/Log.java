/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.util;

/** Android Log 桩：写到 stderr（宿主的日志通道），带等级与 tag 前缀。 */
public final class Log {

    public static final int VERBOSE = 2;
    public static final int DEBUG = 3;
    public static final int INFO = 4;
    public static final int WARN = 5;
    public static final int ERROR = 6;
    public static final int ASSERT = 7;

    private Log() { }

    public static int v(String tag, String msg) { return println('V', tag, msg, null); }
    public static int v(String tag, String msg, Throwable tr) { return println('V', tag, msg, tr); }
    public static int d(String tag, String msg) { return println('D', tag, msg, null); }
    public static int d(String tag, String msg, Throwable tr) { return println('D', tag, msg, tr); }
    public static int i(String tag, String msg) { return println('I', tag, msg, null); }
    public static int i(String tag, String msg, Throwable tr) { return println('I', tag, msg, tr); }
    public static int w(String tag, String msg) { return println('W', tag, msg, null); }
    public static int w(String tag, String msg, Throwable tr) { return println('W', tag, msg, tr); }
    public static int w(String tag, Throwable tr) { return println('W', tag, "", tr); }
    public static int e(String tag, String msg) { return println('E', tag, msg, null); }
    public static int e(String tag, String msg, Throwable tr) { return println('E', tag, msg, tr); }

    public static int wtf(String tag, String msg) { return println('A', tag, msg, null); }
    public static int wtf(String tag, String msg, Throwable tr) { return println('A', tag, msg, tr); }
    public static int wtf(String tag, Throwable tr) { return println('A', tag, "", tr); }

    public static boolean isLoggable(String tag, int level) {
        return level >= DEBUG;
    }

    /** 与安卓语义一致：把 throwable 的栈渲染成字符串（部分蜘蛛用它做事后处理）。 */
    public static String getStackTraceString(Throwable tr) {
        if (tr == null) return "";
        java.io.StringWriter sw = new java.io.StringWriter();
        tr.printStackTrace(new java.io.PrintWriter(sw));
        return sw.toString();
    }

    public static int println(int priority, String tag, String msg) {
        return println(' ', tag, msg, null);
    }

    private static int println(char level, String tag, String msg, Throwable tr) {
        StringBuilder sb = new StringBuilder(64);
        sb.append('[').append(level).append('/').append(tag == null ? "" : tag).append("] ").append(msg == null ? "" : msg);
        System.err.println(sb);
        if (tr != null) System.err.println(getStackTraceString(tr));
        System.err.flush();
        return sb.length();
    }
}
