// VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）
package android.app;

/**
 * 桌面端没有 TV/车载/手表模式：一律按「普通桌面」回答。
 *
 * <p>补充原因（P1 真实样本）：部分 TVBox 蜘蛛在 {@code Init.init()} 里用
 * {@code getSystemService(UI_MODE_SERVICE)} 判断是否 TV 设备，缺类会直接
 * {@code ClassNotFoundException} 让整条链路在初始化阶段失败。
 */
public class UiModeManager {
    public static final int MODE_NIGHT_NO = 1;
    public static final int MODE_NIGHT_YES = 2;
    public static final int MODE_NIGHT_AUTO = 0;
    /** API 31 起：默认跟随系统。 */
    public static final int MODE_NIGHT_AUTO_BATTERY = 3;

    public static final int MODE_NORMAL = 1;
    public static final int MODE_DESK = 2;
    public static final int MODE_CAR = 3;
    public static final int MODE_TELEVISION = 4;
    public static final int MODE_APPLIANCE = 5;
    public static final int MODE_WATCH = 6;
    public static final int MODE_VR_HEADSET = 7;

    public int getCurrentModeType() {
        return MODE_NORMAL;
    }

    public int getNightMode() {
        return MODE_NIGHT_NO;
    }

    public void setNightMode(int mode) {
        // 桌面端不接管系统主题
    }

    public int getApplicationNightMode() {
        return MODE_NIGHT_NO;
    }

    public void setApplicationNightMode(int mode) {
    }

    public boolean isNightMode() {
        return false;
    }
}
