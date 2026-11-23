/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.graphics;

/**
 * Matrix 桩：只记录 3x3 仿射变换，不做实际像素重采样（Bitmap 缩放用最近邻）。
 * 蜘蛛侧常见用途是算海报裁切区域，调用可返回而不抛异常即可。
 */
public class Matrix {

    public enum ScaleToFit { FILL, START, CENTER, END }

    private float[] m = {1f, 0f, 0f, 0f, 1f, 0f, 0f, 0f, 1f};

    public Matrix() { }

    public Matrix(Matrix src) {
        if (src != null) m = src.m.clone();
    }

    public void reset() {
        m = new float[]{1f, 0f, 0f, 0f, 1f, 0f, 0f, 0f, 1f};
    }

    public void set(Matrix src) {
        if (src != null) m = src.m.clone();
    }

    public void setScale(float sx, float sy) {
        m[0] = sx;
        m[4] = sy;
    }

    public void setScale(float sx, float sy, float px, float py) {
        setScale(sx, sy);
        m[2] = px - sx * px;
        m[5] = py - sy * py;
    }

    public void setTranslate(float dx, float dy) {
        m[2] = dx;
        m[5] = dy;
    }

    public void postScale(float sx, float sy) {
        m[0] *= sx;
        m[4] *= sy;
    }

    public void postScale(float sx, float sy, float px, float py) {
        postScale(sx, sy);
        m[2] = px - sx * px;
        m[5] = py - sy * py;
    }

    public void postTranslate(float dx, float dy) {
        m[2] += dx;
        m[5] += dy;
    }

    public void preScale(float sx, float sy) {
        postScale(sx, sy);
    }

    public void preTranslate(float dx, float dy) {
        postTranslate(dx, dy);
    }

    public boolean setRectToRect(Rect src, Rect dst, ScaleToFit mode) {
        if (src == null || dst == null || src.width() == 0 || src.height() == 0) return false;
        float sx = dst.width() / (float) src.width();
        float sy = dst.height() / (float) src.height();
        float s = Math.min(sx, sy);
        setScale(s, s);
        m[2] = dst.left - src.left * s;
        m[5] = dst.top - src.top * s;
        return true;
    }

    public boolean mapRect(Rect rect) {
        if (rect == null) return false;
        float left = m[0] * rect.left + m[2];
        float top = m[4] * rect.top + m[5];
        float right = m[0] * rect.right + m[2];
        float bottom = m[4] * rect.bottom + m[5];
        rect.set(Math.round(left), Math.round(top), Math.round(right), Math.round(bottom));
        return true;
    }

    public void getValues(float[] values) {
        if (values == null) return;
        System.arraycopy(m, 0, values, 0, Math.min(9, values.length));
    }

    public void setValues(float[] values) {
        if (values == null) return;
        System.arraycopy(values, 0, m, 0, Math.min(9, values.length));
    }
}
