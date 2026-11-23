/* VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * 见 jvm/compat/README.md。
 */
package android.graphics;

/**
 * Bitmap 桩：只承载宽高/像素数组，不做真正的图形加速。
 *
 * 蜘蛛多用于「把海报字节流转成 Bitmap 再算尺寸」「QR 登录时截屏」这类旁路需求，
 * 像素读写在内存里真实发生（getPixels/setPixels 可用），仅 compress() 不做编码。
 */
public class Bitmap {

    public enum Config { ALPHA_8, RGB_565, ARGB_4444, ARGB_8888, RGBA_F16, HARDWARE }

    public enum CompressFormat { JPEG(0), PNG(1), WEBP(2);

        CompressFormat(int nativeInt) { this.nativeInt = nativeInt; }
        final int nativeInt;
    }

    public static final int DENSITY_NONE = 0;

    private int width;
    private int height;
    private Config config;
    private int[] pixels;
    private boolean recycled;
    private boolean mutable;
    private int density = DENSITY_NONE;

    private Bitmap(int width, int height, Config config) {
        this.width = Math.max(0, width);
        this.height = Math.max(0, height);
        this.config = config == null ? Config.ARGB_8888 : config;
        this.mutable = true;
        this.pixels = new int[this.width * this.height];
    }

    public static Bitmap createBitmap(int width, int height, Config config) {
        return new Bitmap(width, height, config);
    }

    public static Bitmap createBitmap(int width, int height, Config config, boolean hasAlpha) {
        return new Bitmap(width, height, config);
    }

    public static Bitmap createBitmap(Bitmap src) {
        if (src == null) return null;
        Bitmap out = new Bitmap(src.width, src.height, src.config);
        out.pixels = src.pixels.clone();
        return out;
    }

    public static Bitmap createBitmap(Bitmap src, int x, int y, int w, int h) {
        return createScaledBitmap(src, w, h, true);
    }

    public static Bitmap createBitmap(Bitmap source, int x, int y, int width, int height,
                                      Matrix m, boolean filter) {
        return createScaledBitmap(source, width, height, filter);
    }

    public static Bitmap createScaledBitmap(Bitmap src, int dstWidth, int dstHeight, boolean filter) {
        if (src == null) return null;
        // 最近邻缩放：只保证尺寸正确，颜色不做插值（P1 视需要再补齐）
        Bitmap out = new Bitmap(dstWidth, dstHeight, src.config);
        if (src.width <= 0 || src.height <= 0 || dstWidth <= 0 || dstHeight <= 0) return out;
        for (int y = 0; y < dstHeight; y++) {
            int sy = y * src.height / dstHeight;
            for (int x = 0; x < dstWidth; x++) {
                int sx = x * src.width / dstWidth;
                out.pixels[y * dstWidth + x] = src.pixels[sy * src.width + sx];
            }
        }
        return out;
    }

    public static Bitmap createBitmap(int[] colors, int offset, int stride, int width, int height, Config config) {
        Bitmap out = new Bitmap(width, height, config);
        for (int y = 0; y < height; y++) {
            System.arraycopy(colors, offset + y * stride, out.pixels, y * width, Math.min(width, Math.max(0, colors.length - offset - y * stride)));
        }
        return out;
    }

    public static Bitmap createBitmap(int[] colors, int width, int height, Config config) {
        Bitmap out = new Bitmap(width, height, config);
        System.arraycopy(colors, 0, out.pixels, 0, Math.min(colors.length, out.pixels.length));
        return out;
    }

    public int getWidth() { return width; }

    public int getHeight() { return height; }

    public Config getConfig() { return config; }

    public int getRowBytes() { return width * 4; }

    public int getByteCount() { return width * height * 4; }

    public int getAllocationByteCount() { return getByteCount(); }

    public int getDensity() { return density; }

    public void setDensity(int density) { this.density = density; }

    public boolean isMutable() { return mutable; }

    public boolean isRecycled() { return recycled; }

    public void recycle() {
        recycled = true;
        pixels = null;
    }

    public byte[] getNinePatchChunk() { return null; }

    public void getPixels(int[] outPixels, int offset, int stride, int x, int y, int width, int height) {
        if (outPixels == null || pixels == null) return;
        for (int row = 0; row < height; row++) {
            int src = (y + row) * this.width + x;
            int dst = offset + row * stride;
            for (int col = 0; col < width; col++) {
                if (src + col < pixels.length && dst + col < outPixels.length) {
                    outPixels[dst + col] = pixels[src + col];
                }
            }
        }
    }

    public void setPixels(int[] outPixels, int offset, int stride, int x, int y, int width, int height) {
        if (outPixels == null || pixels == null) return;
        for (int row = 0; row < height; row++) {
            int dst = (y + row) * this.width + x;
            int src = offset + row * stride;
            for (int col = 0; col < width; col++) {
                if (dst + col < pixels.length && src + col < outPixels.length) {
                    pixels[dst + col] = outPixels[src + col];
                }
            }
        }
    }

    public int getPixel(int x, int y) {
        if (pixels == null || x < 0 || y < 0 || x >= width || y >= height) return 0;
        return pixels[y * width + x];
    }

    public void setPixel(int x, int y, int color) {
        if (pixels == null || x < 0 || y < 0 || x >= width || y >= height) return;
        pixels[y * width + x] = color;
    }

    public Bitmap copy(Config config, boolean isMutable) {
        Bitmap out = new Bitmap(width, height, config);
        if (pixels != null) out.pixels = pixels.clone();
        out.mutable = isMutable;
        return out;
    }

    public void eraseColor(int c) {
        if (pixels == null) return;
        java.util.Arrays.fill(pixels, c);
    }

    public boolean compress(CompressFormat format, int quality, java.io.OutputStream stream) {
        // P1：真实编码（PNG/JPEG）尚未实现；调用方通常只关心返回值。
        return false;
    }

    public boolean hasAlpha() {
        return false;
    }

    public void prepareToDraw() {
        // no-op
    }
}
