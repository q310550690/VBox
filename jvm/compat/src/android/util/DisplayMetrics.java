/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.util;

/**
 * DisplayMetrics 桩：桌面上没有真实屏幕，用一块固定的「TV 屏」指标。
 * 蜘蛛常拿它算列数/图宽（widthPixels/density），数值要接近安卓 TV 才不会排版错乱。
 */
public class DisplayMetrics {

    public static final int DENSITY_LOW = 120;
    public static final int DENSITY_MEDIUM = 160;
    public static final int DENSITY_HIGH = 240;
    public static final int DENSITY_XHIGH = 320;
    public static final int DENSITY_XXHIGH = 480;
    public static final int DENSITY_DEFAULT = DENSITY_MEDIUM;

    public int widthPixels;
    public int heightPixels;
    public float density;
    public int densityDpi;
    public float scaledDensity;
    public float xdpi;
    public float ydpi;

    public DisplayMetrics() {
        setToDefaults();
    }

    public void setToDefaults() {
        widthPixels = 1920;
        heightPixels = 1080;
        density = 2.0f;
        densityDpi = DENSITY_XXHIGH;
        scaledDensity = density;
        xdpi = 320f;
        ydpi = 320f;
    }

    public void setTo(DisplayMetrics other) {
        if (other == null) return;
        widthPixels = other.widthPixels;
        heightPixels = other.heightPixels;
        density = other.density;
        densityDpi = other.densityDpi;
        scaledDensity = other.scaledDensity;
        xdpi = other.xdpi;
        ydpi = other.ydpi;
    }

    @Override
    public String toString() {
        return "DisplayMetrics{" + widthPixels + "x" + heightPixels + ", density=" + density + ", dpi=" + densityDpi + "}";
    }
}
