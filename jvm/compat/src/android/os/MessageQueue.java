/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.os;

/**
 * MessageQueue 桩：桌面端的 Handler 用调度线程而非消息队列（见 Handler.java），
 * 因此这里只提供与安卓一致的**空壳**，让 Looper.myQueue()/getQueue() 的调用方不 NPE。
 */
public class MessageQueue {

    MessageQueue() { }

    public boolean isIdle() {
        return true;
    }

    public void quit() { }

    public void quitSafely() { }
}
