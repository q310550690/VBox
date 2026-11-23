/*
 * VBox JVM 兼容层自研实现（AGPL-3.0-only；不属于 jvm/upstream 的 MIT 上游代码）
 *
 * dex -> jar 的**进程内**转换器：直接调用 dex2jar 的 `Dex2Asm` + 自写 `ClassVisitorFactory`，
 * 把每个 class 写进 `ZipOutputStream`。
 *
 * 存在理由：dex2jar 的 `Dex2jarCmd` 走 JDK zipfs（`FileSystems.newFileSystem`），
 * 在受限环境（进程的 `Files.isWritable` 恒 false，例如被沙箱/安全软件包裹时）会误判目标只读，
 * 转换失败且只留下 0 个 class 的假产物；改用普通 `FileOutputStream` 完全绕开 zipfs。
 * 顺带省掉每次转换拉起子 JVM 的开销（首屏更稳）。
 */
package tvbox.dex;

import com.googlecode.d2j.dex.ClassVisitorFactory;
import com.googlecode.d2j.dex.Dex2Asm;
import com.googlecode.d2j.node.DexFileNode;
import com.googlecode.d2j.reader.DexFileReader;

import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.Opcodes;

import java.io.BufferedOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.BufferedInputStream;
import java.util.HashSet;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import java.util.zip.ZipOutputStream;

public final class DexStreamConverter {

    /** 上一次 `convert` 里失败的 class 数（调用方据此决定是否回退 Dex2jarCmd）。 */
    private static volatile int lastFailed = 0;

    /** 上一次 `convert` 失败的 class 数。 */
    public static int lastFailed() {
        return lastFailed;
    }

    private DexStreamConverter() {
    }

    /** 文件头是不是 zip（`PK\x03\x04`）。 */
    public static boolean isZip(File f) {
        if (f == null || !f.isFile() || f.length() < 4) return false;
        try (java.io.RandomAccessFile raf = new java.io.RandomAccessFile(f, "r")) {
            byte[] b = new byte[4];
            raf.readFully(b);
            return b[0] == 'P' && b[1] == 'K' && b[2] == 3 && b[3] == 4;
        } catch (Throwable t) {
            return false;
        }
    }

    /** 文件是不是裸 dex（`dex\n` / `dey\n` 魔数）。 */
    public static boolean isDex(File f) {
        if (f == null || !f.isFile() || f.length() < 8) return false;
        try (java.io.RandomAccessFile raf = new java.io.RandomAccessFile(f, "r")) {
            byte[] b = new byte[8];
            raf.readFully(b);
            return (b[0] == 'd' && b[1] == 'e' && b[2] == 'x' && b[3] == '\n')
                    || (b[0] == 'd' && b[1] == 'e' && b[2] == 'y' && b[3] == '\n');
        } catch (Throwable t) {
            return false;
        }
    }

    /**
     * 源是 zip 时取出其中第一个 `.dex` 写成裸 dex（**容忍中央目录尺寸/CRC 不匹配**）。
     *
     * @return 可用的裸 dex 文件（src 本身是 dex 时原样返回；失败返回 null）
     */
    public static File extractDexIfZip(File src, File dexOut) {
        if (src == null || !src.isFile()) return null;
        if (isDex(src)) return src;
        if (!isZip(src)) return null;
        if (isDex(dexOut)) return dexOut;
        return materialize(src, dexOut) ? dexOut : null;
    }

    /**
     * @return 写出的 class 数（0 表示源不是有效 dex / 全部转换失败）
     */
    public static int convert(File src, File dst) throws Exception {
        final DexFileNode file = new DexFileNode();
        // 与 dex2jar 上游 `Dex2jar` 的默认 reader 配置保持一致：
        //   SKIP_DEBUG(1) 不保留调试信息（否则多出 LocalVariableTable、且寄存器分配会多占 JVM locals）；
        //   IGNORE_READ_EXCEPTION(32) 容忍个别坏结构，继续翻译。
        // 上游 Dex2jar 构造器里写的是 `readerConfig |= 1`，doTranslate 再 `| 32`。
        new DexFileReader(src).accept(file, 1 | 32);

        System.err.println("[dex-stream] 开始转换 " + src.getName() + "（" + src.length() + " B）");
        final int[] written = {0};
        final Throwable[] firstError = {null};
        try (ZipOutputStream zip = new ZipOutputStream(new BufferedOutputStream(new FileOutputStream(dst)))) {
            final ClassVisitorFactory factory = new ClassVisitorFactory() {
                @Override
                public ClassVisitor create(final String name) {
                    // 只算 maxStack/maxLocals，不算 StackMapTable：
                    //   ① 帧计算（COMPUTE_FRAMES）对 600+ 类的大 dex 极吃内存，256MB 宿主会直接 OOM；
                    //   ② 宿主以 `-noverify` 启动，class 加载时不做校验，没有 StackMapTable 也能跑；
                    //   ③ 真要恢复严格校验，构建期可用 JarRewriter 重算帧。
                    return new ClassVisitor(Opcodes.ASM9, new ClassWriter(ClassWriter.COMPUTE_MAXS)) {
                        @Override
                        public void visitEnd() {
                            super.visitEnd();
                            try {
                                byte[] bytes = ((ClassWriter) cv).toByteArray();
                                synchronized (zip) {
                                    zip.putNextEntry(new ZipEntry(name + ".class"));
                                    zip.write(bytes);
                                    zip.closeEntry();
                                    written[0]++;
                                    if (written[0] % 100 == 0) {
                                        System.err.println("[dex-stream] 已写出 " + written[0] + " 个 class");
                                    }
                                }
                            } catch (Throwable t) {
                                if (firstError[0] == null) {
                                    firstError[0] = t;
                                }
                            }
                        }
                    };
                }
            };

            // 逐类转换：dex2jar 的 `convertDex` 遇到第一个坏类就整体抛异常——
            // 实测一声 `Fail on Op invoke-direct index 12` 让 1356 类的 dex 只写出 86 类。
            // 这里自己遍历 class 列表，单类失败只记日志、继续下一个，最后把失败数交给调用方。
            Dex2Asm asm = new Dex2Asm();
            int failed = 0;
            for (com.googlecode.d2j.node.DexClassNode cn : file.clzs) {
                try {
                    asm.convertClass(cn, factory);
                } catch (Throwable t) {
                    failed++;
                    if (failed <= 20) {
                        System.err.println("[dex-stream] 类转换失败（继续）: " + cn.className + " → " + t);
                    }
                }
            }
            lastFailed = failed;
            if (failed > 0) {
                System.err.println("[dex-stream] 共 " + failed + " 个类转换失败（已写出 " + written[0] + " 个）");
            }
            if (written[0] == 0 && firstError[0] != null) {
                Throwable t = firstError[0];
                throw t instanceof Exception ? (Exception) t : new RuntimeException(t);
            }
        }
        return written[0];
    }

    // ---------------------------------------------------------------------
    // 宽容解包：加固壳写出的载荷 zip 常常「中央目录声明的尺寸/CRC 与实际数据不符」
    // （实测 awdatabase.db: 声明 4,089,804 B，deflate 实际只解出 4,082,020 B，ZipFile 直接 Bad CRC-32）。
    // 这里只信中央目录的「偏移/压缩长度」，自己 inflate，只校验 dex 魔数。
    // ---------------------------------------------------------------------

    /** 从 zip 里取出第一个 `.dex` 写成裸 dex（只校验魔数，不校验 CRC/尺寸声明）。 */
    private static boolean materialize(File archive, File dexOut) {
        java.io.RandomAccessFile raf = null;
        try {
            raf = new java.io.RandomAccessFile(archive, "r");
            long len = raf.length();
            int tailLen = (int) Math.min(len, 66000L);
            byte[] tail = new byte[tailLen];
            raf.seek(len - tailLen);
            raf.readFully(tail);
            int eocd = -1;
            for (int i = tailLen - 22; i >= 0; i--) {
                if ((tail[i] & 0xff) == 0x50 && (tail[i + 1] & 0xff) == 0x4b
                        && (tail[i + 2] & 0xff) == 0x05 && (tail[i + 3] & 0xff) == 0x06) {
                    eocd = i;
                    break;
                }
            }
            if (eocd < 0) return false;
            int total = u16(tail, eocd + 10);
            long cdOff = u32(tail, eocd + 16);
            if (cdOff <= 0 || cdOff >= len) return false;

            for (int n = 0; n < total; n++) {
                raf.seek(cdOff);
                byte[] cd = new byte[46];
                raf.readFully(cd);
                if (u32(cd, 0) != 0x02014b50L) return false;
                int method = u16(cd, 10);
                long compSize = u32(cd, 20);
                int nameLen = u16(cd, 28);
                int extraLen = u16(cd, 30);
                int commentLen = u16(cd, 32);
                long localOff = u32(cd, 42);
                byte[] nameBytes = new byte[nameLen];
                raf.readFully(nameBytes);
                String name = new String(nameBytes, "UTF-8");
                if (!name.endsWith(".dex")) {
                    cdOff += 46 + nameLen + extraLen + commentLen;
                    continue;
                }
                raf.seek(localOff);
                byte[] lh = new byte[30];
                raf.readFully(lh);
                if (u32(lh, 0) != 0x04034b50L) return false;
                int lName = u16(lh, 26);
                int lExtra = u16(lh, 28);
                long dataOff = localOff + 30 + lName + lExtra;
                if (compSize <= 0 || dataOff + compSize > len) return false;
                byte[] comp = new byte[(int) compSize];
                raf.seek(dataOff);
                raf.readFully(comp);
                byte[] plain;
                if (method == 0) {
                    plain = comp;
                } else if (method == 8) {
                    java.util.zip.Inflater inf = new java.util.zip.Inflater(true);
                    try {
                        inf.setInput(comp);
                        java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream(comp.length * 4);
                        byte[] buf = new byte[65536];
                        while (!inf.finished()) {
                            int got = inf.inflate(buf);
                            if (got == 0) {
                                if (inf.needsInput() || inf.needsDictionary()) break;
                            }
                            bos.write(buf, 0, got);
                        }
                        plain = bos.toByteArray();
                    } finally {
                        inf.end();
                    }
                } else {
                    return false;
                }
                if (plain.length < 8) return false;
                boolean ok = (plain[0] == 'd' && plain[1] == 'e' && plain[2] == 'x' && plain[3] == '\n')
                        || (plain[0] == 'd' && plain[1] == 'e' && plain[2] == 'y' && plain[3] == '\n');
                if (!ok) return false;
                java.nio.file.Files.write(dexOut.toPath(), plain);
                System.err.println("[dex-stream] 载荷 dex 已按中央目录自行解出: " + archive.getName()
                        + " → " + plain.length + " B（容忍 zip 元数据不匹配）");
                return true;
            }
            return false;
        } catch (Throwable t) {
            System.err.println("[dex-stream] 自行解 dex 失败: " + t);
            return false;
        } finally {
            if (raf != null) {
                try { raf.close(); } catch (Throwable ignored) { }
            }
        }
    }

    /**
     * 把源 jar 里除 class 之外的文件（`.so` / assets / 配置 json 等）补进产物。
     *
     * 加固壳的 native 桥要在「被加载的 jar」里找 `assets/*.so` 才能起模拟器；
     * 而 dex→jar 只产出 class，不复制资源，于是桥会报「jar 内没有可用的 ARM .so」。
     * 这里把资源原样搬过去（跳过 classes*.dex / META-INF 签名与 manifest）。
     *
     * @return 补进去的条目数（0 表示源里没有资源或失败）
     */
    public static int copyResources(File src, File out) {
        if (src == null || out == null || !src.isFile() || !out.isFile() || !isZip(src)) return 0;
        Set<String> existing = new HashSet<String>();
        boolean hasResource = false;
        try (ZipInputStream in = new ZipInputStream(new BufferedInputStream(new java.io.FileInputStream(src)))) {
            ZipEntry e;
            while ((e = in.getNextEntry()) != null) {
                if (e.isDirectory()) continue;
                if (!isResourceEntry(e.getName())) { hasResource = true; }
            }
        } catch (Throwable t) {
            System.err.println("[dex-stream] 扫描源 jar 资源失败: " + t);
            return 0;
        }
        if (!hasResource) return 0;
        try (ZipInputStream in = new ZipInputStream(new BufferedInputStream(new java.io.FileInputStream(out)))) {
            ZipEntry e;
            while ((e = in.getNextEntry()) != null) existing.add(e.getName());
        } catch (Throwable t) {
            System.err.println("[dex-stream] 读取产物条目失败: " + t);
            return 0;
        }
        File tmp = new File(out.getParentFile(), out.getName() + ".merge.tmp");
        int copied = 0;
        try (ZipOutputStream zos = new ZipOutputStream(new java.io.BufferedOutputStream(new java.io.FileOutputStream(tmp)))) {
            // ① 先原样搬回产物里的 class
            try (ZipInputStream in = new ZipInputStream(new BufferedInputStream(new java.io.FileInputStream(out)))) {
                ZipEntry e;
                byte[] buf = new byte[64 * 1024];
                while ((e = in.getNextEntry()) != null) {
                    if (e.isDirectory()) continue;
                    zos.putNextEntry(new ZipEntry(e.getName()));
                    int n;
                    while ((n = in.read(buf)) > 0) zos.write(buf, 0, n);
                    zos.closeEntry();
                }
            }
            // ② 再补源里的资源
            try (ZipInputStream in = new ZipInputStream(new BufferedInputStream(new java.io.FileInputStream(src)))) {
                ZipEntry e;
                byte[] buf = new byte[64 * 1024];
                while ((e = in.getNextEntry()) != null) {
                    if (e.isDirectory()) continue;
                    String name = e.getName();
                    if (isResourceEntry(name) || existing.contains(name)) continue;
                    zos.putNextEntry(new ZipEntry(name));
                    int n;
                    while ((n = in.read(buf)) > 0) zos.write(buf, 0, n);
                    zos.closeEntry();
                    copied++;
                }
            }
        } catch (Throwable t) {
            System.err.println("[dex-stream] 补资源失败: " + t);
            try { java.nio.file.Files.deleteIfExists(tmp.toPath()); } catch (Throwable ignored) { }
            return 0;
        }
        try {
            java.nio.file.Files.deleteIfExists(out.toPath());
            java.nio.file.Files.move(tmp.toPath(), out.toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING);
        } catch (Throwable t) {
            System.err.println("[dex-stream] 资源合并回写失败: " + t);
            return 0;
        }
        System.err.println("[dex-stream] 补资源 " + copied + " 项（源 jar 的 assets/*.so 等）");
        return copied;
    }

    /** class 本体与签名文件不算资源：转换产物里由 dex2jar 重新生成/不应携带。 */
    private static boolean isResourceEntry(String name) {
        if (name.startsWith("META-INF/")) return true;
        if (name.startsWith("classes") && name.endsWith(".dex")) return true;
        if (name.endsWith(".class")) return true;
        return false;
    }

    private static int u16(byte[] b, int off) {
        return (b[off] & 0xff) | ((b[off + 1] & 0xff) << 8);
    }

    private static long u32(byte[] b, int off) {
        return (u16(b, off) | ((long) u16(b, off + 2) << 16)) & 0xffffffffL;
    }
}
