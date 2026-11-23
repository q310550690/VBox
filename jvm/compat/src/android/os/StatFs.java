// VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）
package android.os;

import java.io.File;

/** 桌面端用 {@link File} 的空间信息回答；蜘蛛通常只用来判断「剩余空间够不够」。 */
public class StatFs {
    private final String path;

    public StatFs(String path) {
        this.path = path == null || path.isEmpty() ? "." : path;
    }

    private File file() {
        File f = new File(path);
        // 传入的可能是文件路径；取父目录的空间信息更贴近 Android 语义
        return f.isFile() ? f.getParentFile() : f;
    }

    public void restat(String path) {
    }

    public int getBlockSize() {
        return 4096;
    }

    public long getBlockCount() {
        File f = file();
        return f == null ? 0L : f.getTotalSpace() / 4096L;
    }

    public long getFreeBlocks() {
        File f = file();
        return f == null ? 0L : f.getUsableSpace() / 4096L;
    }

    public long getAvailableBlocks() {
        return getFreeBlocks();
    }

    public long getAvailableBytes() {
        File f = file();
        return f == null ? 0L : f.getUsableSpace();
    }

    public long getFreeBytes() {
        return getAvailableBytes();
    }

    public long getBlockSizeLong() {
        return getBlockSize();
    }

    /** API 18+ 的 *Long 变体（Picasso/Glide 一类库会调）。 */
    public long getBlockCountLong() {
        return getBlockCount();
    }

    public long getAvailableBlocksLong() {
        return getAvailableBlocks();
    }

    public long getFreeBlocksLong() {
        return getFreeBlocks();
    }

    public long getFreeBytesLong() {
        return getFreeBytes();
    }

    public long getAvailableBytesLong() {
        return getAvailableBytes();
    }

    public long getTotalBytesLong() {
        return getTotalBytes();
    }

    public long getTotalBytes() {
        File f = file();
        return f == null ? 0L : f.getTotalSpace();
    }
}
