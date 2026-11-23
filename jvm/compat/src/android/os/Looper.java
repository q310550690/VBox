/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.os;

/**
 * Looper 桩：桌面 JVM 没有安卓消息循环。
 * Handler 在该兼容层里用调度线程实现（见 android/os/Handler.java），
 * 因此 Looper 只需「存在且可获取主线程实例」，loop() 不得真阻塞调用线程。
 */
public final class Looper {

    private static final Looper MAIN = new Looper(true);
    private static final ThreadLocal<Looper> CURRENT = new ThreadLocal<Looper>();

    private final boolean main;
    private volatile boolean quit;

    private Looper(boolean main) {
        this.main = main;
    }

    public static Looper getMainLooper() {
        return MAIN;
    }

    public static Looper myLooper() {
        Looper l = CURRENT.get();
        return l == null ? MAIN : l;
    }

    public static void prepare() {
        CURRENT.set(new Looper(false));
    }

    public static void prepareMainLooper() {
        CURRENT.set(MAIN);
    }

    public static void loop() {
        // 桌面版没有消息队列：立即返回，避免宿主线程被永久阻塞。
    }

    public boolean isCurrentThread() {
        return myLooper() == this;
    }

    public Thread getThread() {
        return Thread.currentThread();
    }

    public void quit() {
        quit = true;
    }

    public void quitSafely() {
        quit = true;
    }

    public boolean isQuitting() {
        return quit;
    }

    public MessageQueue getQueue() {
        return null;
    }

    public static MessageQueue myQueue() {
        return null;
    }

    @Override
    public String toString() {
        return "Looper{" + (main ? "main" : "worker") + "}";
    }
}
