package dalvik.system;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.URL;
import java.net.URLClassLoader;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Enumeration;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/**
 * BaseDexClassLoader stub —— 安卓动态 dex 加载器的基类（等效 DexClassLoader 的血统）。
 *
 * <p><b>为什么需要这一族类：</b>不少「加固 / 壳」型蜘蛛 jar（典型如含
 * {@code com.github.catvod.spider.DexNative} 的那批 {@code *Guard} 类）走的是
 * <b>安卓原生加载链</b>：
 * <pre>
 *   Init.init(Context) → DexNative.getLoader(ctx)      // native 方法
 *                          └─ 解密 assets/xxx.guard、new DexClassLoader(...)
 *   BaseSpiderGuard.&lt;init&gt;() → Init.getSpider() → DexNative.proxyInvoke(...)
 * </pre>
 *
 * <p><b>「壳通解」：裸 .dex 现在能真正加载了（勿回退）</b>
 * <p>壳把 {@code assets/<X>.guard} 交给随包的 ARM {@code .so} 解密，得到明文 dex
 * （在 x64 上这一步由 unidbg 原生桥代跑，见 resources/jvm/native-bridge/），
 * 随后走 {@code new DexClassLoader(dexPath, …)}。
 * 本类对**裸 .dex**（以及「只含 classes.dex 的归档」）就地做一次
 * <b>运行时 dex2jar</b>（与宿主 JarSpiderBridge 同一条 d2j 链，按 dex 内容 md5 缓存），
 * 再用 {@link URLClassLoader} 加载 —— 于是任何「native 解密自己 dex」的壳都不再需要
 * 逐壳实测。
 * <p><b>转换「代价只付一次」</b> —— 产物写 `&lt;md5&gt;.part.jar` 后原子改名，
 * 输出落日志文件（不走管道）；本 JVM 被宿主 kill 后，子进程写完的 part 会在下次进源时
 * 直接**收编**（见 {@link #dexToJar} 的注释），不会再让用户对着「首次加载」等第二遍。
 *
 * <p><b>（新订阅 96 源全空）「产物收尾」两步（勿删）</b>
 * 实测解密产物是 **zip**（`classes.dex` + `assets/libwex_v7.so` + `assets/libwex_v8.so`），
 * 而 dex2jar 只产出 .class ⇒ 载荷类 `<clinit>` 里
 * {@code InitOrigin.classLoader().getResourceAsStream("assets/libwex_v8.so")} 恒为 null（NPE），
 * 且产物里存在 dex2jar 的「NEW 抽象类」误译（运行时 InstantiationError）。故转换后必须：
 * <ol>
 *   <li>{@link #copyArchiveResources}：把源归档的非 class 条目补进产物（assets 等）；</li>
 *   <li>{@link #repairNewSites}：反射调原生桥的 {@code com.winbox.nativebridge.DexJarRepair}
 *       修 NEW 抽象类（桥缺失则跳过）。</li>
 * </ol>
 * 两者一起用标记文件 `&lt;产物&gt;.fix.ok` 保证只做一次；产物名带版本号
 * {@link #PRODUCT_REV}（旧产物自动失效、重转一次）。
 *
 * <p>依赖三个系统属性（由宿主 {@code JarSpiderBridge.jvmPrefix} 注入）：
 * <ul>
 *   <li>{@code tvbox.d2j.java} = java.exe 路径</li>
 *   <li>{@code tvbox.d2j.cp}   = dex2jar 工具 classpath（resources/jvm/d2j/*.jar）</li>
 *   <li>{@code tvbox.d2j.cache}= 运行时转换产物的缓存目录（缺失则不转换，退回可解释失败）</li>
 * </ul>
 */
public class BaseDexClassLoader extends ClassLoader {

    /**
     * 运行时 dex→jar 产物版本（改「收尾」逻辑时 +1）：
     * 产物名 = `&lt;源归档 md5&gt;.<rev>.jar` —— 老版本产物自然不复用（一次性重转），
     * 避免「旧产物缺 assets / 缺 NEW 修复」被当成好缓存。
     */
    private static final String PRODUCT_REV = ".r2";

    private final String dexPath;
    private final File optimizedDirectory;
    private final String librarySearchPath;

    /** 真正能用的部分：对 jar/zip/apk 用 URLClassLoader 代理。 */
    private URLClassLoader delegate;
    /** 无法处理的条目（转换失败/依赖缺失），首次 loadClass 时用于给出准确报错。 */
    private final List<String> unsupported = new ArrayList<String>();

    public BaseDexClassLoader(String dexPath, File optimizedDirectory,
                              String librarySearchPath, ClassLoader parent) {
        super(parent);
        this.dexPath = dexPath == null ? "" : dexPath;
        this.optimizedDirectory = optimizedDirectory;
        this.librarySearchPath = librarySearchPath;
        this.delegate = buildDelegate(this.dexPath, parent);
        // （壳通解）：把本加载器登记给原生桥 —— 壳的 native 会用 JNI 调用「解密 dex 里的
        //   真实蜘蛛类」的静态方法（实测 某第三方源：`InitOrigin.init(Context)`），而 unidbg 的反射代理按类名
        //   只在壳加载器里找 → CNFE。登记后桥的类工厂会兜底来这里找（原生桥不在时静默跳过）。
        registerWithBridge(this.delegate);
    }

    private static void registerWithBridge(ClassLoader loader) {
        if (loader == null) return;
        try {
            Class<?> nb = Class.forName("com.winbox.nativebridge.NativeBridge");
            nb.getMethod("registerExtraLoader", ClassLoader.class).invoke(null, loader);
        } catch (Throwable ignored) {
            // 普通源（无原生桥）不影响
        }
    }

    /**
     * （壳通解·载荷层）：给运行时转换产物打载荷补丁（反射调用，避免编译期耦合）。
     * 桥不在 classpath / 产物无需补丁 / 打补丁失败 → 一律返回 null（原样加载，行为与补丁前一致）。
     */
    private static File patchRuntime(File jar) {
        try {
            Class<?> facade = Class.forName("com.winbox.nativebridge.NativeBridgeMain");
            Object r = facade.getMethod("patchRuntimeJar", File.class).invoke(null, jar);
            if (!(r instanceof File)) return null;
            File dir = (File) r;
            return dir.isDirectory() || dir.mkdirs() ? dir : null;
        } catch (Throwable ignored) {
            return null;
        }
    }

    public BaseDexClassLoader(String dexPath, String optimizedDirectory,
                              String librarySearchPath, ClassLoader parent) {
        this(dexPath, optimizedDirectory == null ? null : new File(optimizedDirectory),
                librarySearchPath, parent);
    }

    /**
     * 把路径里「JVM 能直接读的归档」抽出来交给 URLClassLoader；
     * **裸 .dex（与只含 classes.dex 的归档）就地运行时转换成 jar**（见类注释）。
     *
     * **按内容判型，不再只看扩展名**。壳把解密产物落盘时常起名叫
     *   `config.db` / `xxx.bin`（实测 某第三方源：`…/cache/code_cache/sharedb/config.db`），
     *   按扩展名判型会把真实 dex 记成 unsupported → 壳里所有 loadClass 全 CNFE。
     */
    private URLClassLoader buildDelegate(String path, ClassLoader parent) {
        if (path == null || path.isEmpty()) return null;
        List<URL> urls = new ArrayList<URL>();
        for (String p : path.split(File.pathSeparator)) {
            String s = p == null ? "" : p.trim();
            if (s.isEmpty()) continue;
            addEntry(urls, s);
        }
        if (urls.isEmpty()) return null;
        try {
            return new URLClassLoader(urls.toArray(new URL[0]), parent);
        } catch (Throwable th) {
            return null;
        }
    }

    /** 单个路径条目 → URL（dex / 只含 dex 的归档先做运行时 dex→jar 转换；其它 zip 直接挂 URL） */
    private void addEntry(List<URL> urls, String raw) {
        File src = locate(raw);
        if (src == null) {
            unsupported.add(raw);
            return;
        }
        boolean dex = isDexFile(src);
        boolean zip = isZipFile(src);
        if (dex || (zip && isDexOnlyArchive(src.getPath()))) {
            File jar = dexToJar(src);
            if (jar != null) {
                try {
                    // （壳通解·载荷层）：解密 dex 里的 real 蜘蛛类自带 ARM native
                    //   （某第三方源：LoadNiMa/MyCrypto/GoProxy），其 `<clinit>` 会 `System.load` 自己拷出来的 .so
                    //   → x64 宿主 UnsatisfiedLinkError → ExceptionInInitializerError → 主页全空。
                    //   先给产物打「载荷补丁」（native → 32 位模拟器；System.load → bridgeLoad），
                    //   并把补丁目录排在**原 jar 之前**（类加载顺序覆盖原类）；无需补丁时为 null。
                    File patched = patchRuntime(jar);
                    if (patched != null) urls.add(patched.toURI().toURL());
                    urls.add(jar.toURI().toURL());
                    return;
                } catch (Throwable ignored) {
                    // 落到 unsupported
                }
            }
            unsupported.add(raw);
            return;
        }
        if (zip) {
            try {
                urls.add(src.toURI().toURL());
                return;
            } catch (Throwable ignored) {
                // 落到 unsupported
            }
        }
        unsupported.add(raw);
    }

    /**
     * 路径解析：先按宿主路径找；找不到再试「来宾 POSIX 路径 → 宿主 rootfs」映射
     * （壳落盘路径两种形态都出现过：宿主 `E:\…` 与来宾 `/data/data/…`）。
     */
    private static File locate(String raw) {
        File f = new File(raw);
        if (f.isFile()) return f;
        try {
            Class<?> pf = Class.forName("com.winbox.nativebridge.PosixFile");
            Object r = pf.getMethod("resolveHost", String.class).invoke(null, raw);
            if (r instanceof File && ((File) r).isFile()) return (File) r;
        } catch (Throwable ignored) {
            // 无原生桥（普通源）时按原路径处理
        }
        return null;
    }

    /** dex 判型：扩展名或魔数（`dex\n` / `dey\n`，覆盖 035~039 各版本） */
    private static boolean isDexFile(File f) {
        if (f.getName().toLowerCase().endsWith(".dex")) return true;
        try (InputStream in = new FileInputStream(f)) {
            byte[] h = new byte[8];
            int n = 0;
            while (n < 8) {
                int r = in.read(h, n, 8 - n);
                if (r < 0) break;
                n += r;
            }
            return n == 8 && h[0] == 'd' && h[1] == 'e' && (h[2] == 'x' || h[2] == 'y') && h[3] == '\n';
        } catch (Throwable t) {
            return false;
        }
    }

    /** zip 判型（jar / apk / 任意扩展名的归档都算） */
    private static boolean isZipFile(File f) {
        try (InputStream in = new FileInputStream(f)) {
            byte[] h = new byte[4];
            int n = 0;
            while (n < 4) {
                int r = in.read(h, n, 4 - n);
                if (r < 0) break;
                n += r;
            }
            return n >= 2 && h[0] == 'P' && h[1] == 'K';
        } catch (Throwable t) {
            return false;
        }
    }

    // ---------------------------------------------------------------- 运行时 dex → jar

    /** 归档里只含 dex（没有 JVM 能直接读的 .class）→ 需要转换。读不出来时按「不是 dex 归档」处理。 */
    private static boolean isDexOnlyArchive(String path) {
        try (ZipFile zf = new ZipFile(path)) {
            boolean hasDex = false;
            Enumeration<? extends ZipEntry> it = zf.entries();
            while (it.hasMoreElements()) {
                String n = it.nextElement().getName();
                if (n.endsWith(".class")) return false;
                if (n.equals("classes.dex") || n.matches("classes\\d+\\.dex")) hasDex = true;
            }
            return hasDex;
        } catch (Throwable t) {
            return false;
        }
    }

    /**
     * 裸 dex / dex 归档 → jar（运行时 dex2jar，按内容 md5 缓存）。
     * 失败（工具缺失、超时、产物为空）返回 null，调用方按「不可加载」记录并在 loadClass 时报准确原因。
     *
     * （「代价只付一次」）：两条与宿主侧同口径的纪律 ——
     *   ① **输出写 `<md5>.part.jar` 并重定向到日志文件**（不再读管道）：本 JVM 被宿主 kill 时管道断裂，
     *      子进程往已断的 stdout 写会抛 IOException，反而把「其实还在跑的转换」带崩；
     *   ② **收编上次会话跑完的 part**：JVM 虽然被杀，但 dex2jar 子进程在 Windows 上不会跟着死，
     *      它写完的 part 一直在磁盘上 —— 下次进源直接改名收编即可，不必再等几分钟重转
     *      （用户诉求：「每次打开软件都要等很久」）。
     */
    private static File dexToJar(File src) {
        if (src == null || !src.isFile()) return null;
        String javaExe = System.getProperty("tvbox.d2j.java", "").trim();
        String d2jCp = System.getProperty("tvbox.d2j.cp", "").trim();
        String cacheProp = System.getProperty("tvbox.d2j.cache", "").trim();
        // cache 目录随宿主工作区给出；进程内转换不需要 javaExe/d2jCp（见下面的 ①）
        if (cacheProp.isEmpty()) return null;
        File cache = new File(cacheProp);
        if (!cache.isDirectory() && !cache.mkdirs()) return null;

        String key = md5Of(src);
        if (key == null) return null;
        File out = new File(cache, key + PRODUCT_REV + ".jar");
        if (out.isFile() && out.length() > 0 && classCountInJar(out) > 0) {
            finalizeProduct(src, out);
            return out;
        }

        File tmp = new File(cache, key + ".part.jar");
        // 收编「上一次会话已经跑完」的转换产物（见方法注释 ②）：省掉整次 dex2jar
        if (isCompleteProduct(tmp) && adoptProduct(tmp, out)) {
            finalizeProduct(src, out);
            return out;
        }

        // ① **子进程转换（首选）**：调用本仓库自己的入口 `tvbox.dex.ConvertMain`
        //    （内部是 `DexStreamConverter`：Dex2Asm + 手写 ZipOutputStream，不碰 zipfs），
        //    放在独立 JVM 里跑 —— 加固壳加载过 unidbg/ARM 模拟器后再做大批量 JIT 转换会被系统终止（实测 exit -1）。
        //    输入若是 zip（加固壳的载荷 jar/自定义容器），先把 dex 取出来（容忍尺寸/CRC 不匹配的写入方）。
        File convertSrc = src;
        if (looksLikeZip(src)) {
            File dex = new File(cache, key + ".dex");
            if (!isPlainDexFile(dex) && !materializeDexLenient(src, dex)) {
                convertSrc = null;
            } else {
                convertSrc = dex;
            }
        }
        File sub = new File(cache, key + ".sub.jar");
        if (convertSrc != null && !javaExe.isEmpty() && !d2jCp.isEmpty()
                && convertViaSubprocess(convertSrc, sub, cache, key, javaExe, d2jCp)) {
            if (sub.renameTo(out) || out.isFile()) {
                finalizeProduct(src, out);
                return out;
            }
            try { sub.delete(); } catch (Throwable ignored) { }
        }
        // ② **进程内转换**：没有可用 java 可执行文件（或子进程失败）时的兜底。
        File inproc = new File(cache, key + ".inproc.jar");
        if (convertSrc != null && convertInProcess(convertSrc, inproc)) {
            if (inproc.renameTo(out) || (out.isFile() && classCountInJar(out) > 0)) {
                finalizeProduct(src, out);
                return out;
            }
            try { inproc.delete(); } catch (Throwable ignored) { }
        }
        if (javaExe.isEmpty() || d2jCp.isEmpty()) {
            System.err.println("[runtime-dex] 进程内转换不可用，且未注入 tvbox.d2j.java/cp，无法回退子进程路线: "
                    + src.getName());
            return null;
        }

        List<String> argv = new ArrayList<String>();
        argv.add(javaExe);
        // 堆按源体积自适应（与宿主 JarSpiderBridge 同口径）：~128MB/1MB，夹在 [512, 1792]
        long mb = Math.max(1L, src.length() / (1024L * 1024L));
        argv.add("-Xmx" + Math.min(1792L, Math.max(512L, mb * 128L + 256L)) + "m");
        argv.add("-XX:+UseSerialGC");
        argv.add("-XX:TieredStopAtLevel=1");
        argv.add("-XX:ReservedCodeCacheSize=32m");
        argv.add("-Dfile.encoding=UTF-8");
        argv.add("-cp");
        argv.add(d2jCp);
        argv.add("com.googlecode.dex2jar.tools.Dex2jarCmd");
        argv.add(src.getAbsolutePath());
        argv.add("-o");
        argv.add(tmp.getAbsolutePath());
        argv.add("--force");
        try {
            ProcessBuilder pb = new ProcessBuilder(argv);
            pb.redirectErrorStream(true);
            // 输出重定向到日志文件（见方法注释 ①）：宿主 kill 本 JVM 后管道必断，落文件才安全；
            //   顺带把 dex2jar 的报错留档（此前只在内存里，JVM 一死就没了）。
            boolean toFile = false;
            try {
                pb.redirectOutput(ProcessBuilder.Redirect.to(new File(cache, key + ".d2j.log")));
                toFile = true;
            } catch (Throwable ignored) {
                // 落文件失败 → 退化为「吞掉输出」，绝不能让它写满 64KB 管道卡死子进程
            }
            Process proc = pb.start();
            if (!toFile) {
                try (InputStream in = proc.getInputStream()) {
                    byte[] buf = new byte[8192];
                    while (in.read(buf) > 0) {
                        // discard
                    }
                } catch (Throwable ignored) {
                }
            }
            if (!proc.waitFor(15, java.util.concurrent.TimeUnit.MINUTES)) {
                proc.destroyForcibly();
                return null;
            }
            if (proc.exitValue() != 0 || !tmp.isFile() || tmp.length() == 0) {
                try { tmp.delete(); } catch (Throwable ignored) { }
                return null;
            }
            if (!tmp.renameTo(out) && !out.isFile()) {
                // 竞态下别的线程已生成同名产物也算成功
                if (!out.isFile()) return null;
            }
            if (!out.isFile() || out.length() <= 0) return null;
            if (classCountInJar(out) == 0) {
                System.err.println("[runtime-dex] dex2jar 产物没有任何 class（目标文件系统拒绝写入 zip），"
                        + "本次不加载: " + src.getName() + "；可在可写环境重试或预热缓存");
                try { out.delete(); } catch (Throwable ignored) { }
                return null;
            }
            finalizeProduct(src, out);
            return out;
        } catch (Throwable t) {
            return null;
        }
    }

    /** 文件头是不是 zip（`PK\x03\x04`）。 */
    private static boolean looksLikeZip(File f) {
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
    private static boolean isPlainDexFile(File f) {
        if (f == null || !f.isFile() || f.length() < 8) return false;
        try (java.io.RandomAccessFile raf = new java.io.RandomAccessFile(f, "r")) {
            byte[] b = new byte[8];
            raf.readFully(b);
            return b[0] == 'd' && b[1] == 'e' && b[2] == 'x' && b[3] == '\n'
                    || b[0] == 'd' && b[1] == 'e' && b[2] == 'y' && b[3] == '\n';
        } catch (Throwable t) {
            return false;
        }
    }

    /**
     * 从 zip 里取出第一个 `.dex` 条目写成裸 dex 文件，**容忍 zip 元数据不匹配**。
     *
     * 加固壳的载荷容器常见两种坏法：① 中央目录声明的 uncompressed size 比真实 deflate 流大
     * （写入方少刷了一块）；② CRC 与数据不一致。`ZipFile` 两种都会抛异常，dex2jar 的 zipfs 同病，
     * 于是载荷「看起来 0 class」。这里按中央目录拿偏移/压缩法，自己 inflate，只校验 dex 魔数。
     *
     * @return true 表示写出了有效 dex
     */
    private static boolean materializeDexLenient(File archive, File dexOut) {
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
                System.err.println("[runtime-dex] 载荷 dex 已按中央目录自行解出: " + archive.getName()
                        + " → " + plain.length + " B（容忍 zip 元数据不匹配）");
                return true;
            }
            return false;
        } catch (Throwable t) {
            System.err.println("[runtime-dex] 自行解 dex 失败: " + t);
            return false;
        } finally {
            if (raf != null) {
                try { raf.close(); } catch (Throwable ignored) { }
            }
        }
    }

    private static int u16(byte[] b, int off) {
        return (b[off] & 0xff) | ((b[off + 1] & 0xff) << 8);
    }

    private static long u32(byte[] b, int off) {
        return (u16(b, off) | ((long) u16(b, off + 2) << 16)) & 0xffffffffL;
    }

    /**
     * **进程内** dex → jar：反射调用 stubs.jar 里的 `tvbox.dex.DexStreamConverter`（AGPL 自研补件）。
     * 它不用 JDK zipfs，因此不受「目标文件系统被判只读」影响；也不需要再拉一个 JVM。
     *
     * @return true 表示已写出至少一个 class 的有效产物
     */
    /**
     * 用独立 JVM 跑 {@code tvbox.dex.ConvertMain}（绕开 zipfs，也避免宿主内的 native 运行时干扰）。
     * 产物写 {@code sub}，成功且 class 数 &gt; 0 返回 true。
     */
    private static boolean convertViaSubprocess(File src, File sub, File cache, String key,
                                                String javaExe, String d2jCp) {
        String self = codeSourceJar();
        String cp = self == null ? d2jCp : d2jCp + File.pathSeparator + self;
        long mb = Math.max(1L, src.length() / (1024L * 1024L));
        long heap = Math.min(1792L, Math.max(384L, mb * 128L + 128L));
        List<String> argv = new ArrayList<String>();
        argv.add(javaExe);
        argv.add("-Xmx" + heap + "m");
        argv.add("-XX:+UseSerialGC");
        argv.add("-XX:TieredStopAtLevel=1");
        argv.add("-Dfile.encoding=UTF-8");
        argv.add("-Dsun.stdout.encoding=UTF-8");
        argv.add("-Dsun.stderr.encoding=UTF-8");
        argv.add("-cp");
        argv.add(cp);
        argv.add("tvbox.dex.ConvertMain");
        argv.add(src.getAbsolutePath());
        argv.add(sub.getAbsolutePath());
        File log = new File(cache, key + ".convert.log");
        try {
            ProcessBuilder pb = new ProcessBuilder(argv);
            pb.redirectErrorStream(true);
            boolean toFile = false;
            try {
                pb.redirectOutput(ProcessBuilder.Redirect.to(log));
                toFile = true;
            } catch (Throwable ignored) {
                // 落文件失败 → 退化为吞掉输出，避免写满管道卡死
            }
            Process proc = pb.start();
            if (!toFile) {
                drain(proc);
            }
            boolean done = proc.waitFor(15, java.util.concurrent.TimeUnit.MINUTES);
            if (!done) {
                proc.destroyForcibly();
                proc.waitFor();
                System.err.println("[runtime-dex] 子进程转换超时（15 分钟）: " + src.getName());
                return false;
            }
            int code = proc.exitValue();
            if (code != 0) {
                System.err.println("[runtime-dex] 子进程转换失败（exit=" + code + "，日志 " + log.getName() + "）: "
                        + src.getName());
                return false;
            }
            return classCountInJar(sub) > 0;
        } catch (Throwable t) {
            System.err.println("[runtime-dex] 子进程转换异常: " + t);
            return false;
        }
    }

    /** 本类所在 jar 的路径（用作子进程 classpath 的一部分；拿不到返回 null）。 */
    private static String codeSourceJar() {
        try {
            java.security.CodeSource cs = BaseDexClassLoader.class.getProtectionDomain().getCodeSource();
            if (cs == null) return null;
            java.net.URL u = cs.getLocation();
            if (u == null) return null;
            File f = new File(u.toURI());
            return f.isFile() && f.getName().endsWith(".jar") ? f.getAbsolutePath() : null;
        } catch (Throwable t) {
            return null;
        }
    }

    /** 不让子进程写满管道（仅在不落文件时用）。 */
    private static void drain(Process proc) {
        try (InputStream in = proc.getInputStream()) {
            byte[] buf = new byte[8192];
            while (in.read(buf) > 0) {
                // discard
            }
        } catch (Throwable ignored) {
            // ignore
        }
    }

    private static boolean convertInProcess(File src, File dst) {
        if (src == null || dst == null) return false;
        try {
            Class<?> c = Class.forName("tvbox.dex.DexStreamConverter");
            Object n = c.getMethod("convert", File.class, File.class).invoke(null, src, dst);
            int written = (n instanceof Integer) ? ((Integer) n).intValue() : 0;
            if (written <= 0 || !dst.isFile() || dst.length() <= 0) {
                try { dst.delete(); } catch (Throwable ignored) { }
                return false;
            }
            System.err.println("[runtime-dex] 进程内转换完成: " + src.getName() + " → " + written + " 个 class");
            return true;
        } catch (Throwable t) {
            try { dst.delete(); } catch (Throwable ignored) { }
            System.err.println("[runtime-dex] 进程内转换不可用（回退子进程路线）: " + t);
            return false;
        }
    }

    /** 产物里可加载的 class 数（受限环境下会产出「有资源、无 class」的假产物）。 */
    private static int classCountInJar(File jar) {
        int n = 0;
        try (ZipFile z = new ZipFile(jar)) {
            Enumeration<? extends ZipEntry> it = z.entries();
            while (it.hasMoreElements()) {
                if (it.nextElement().getName().endsWith(".class")) n++;
            }
        } catch (Throwable ignored) {
            return 0;
        }
        return n;
    }

    /**
     * （新订阅 96 源全空）**产物收尾**：补 assets + 修 NEW 抽象类（见类注释）。
     * 用标记文件 `&lt;产物&gt;.fix.ok` 保证只做一次；任一步失败都只记日志，不影响产物可用性。
     */
    private static void finalizeProduct(File src, File out) {
        finalizeProductBody(src, out);
        exportPayloadProduct(out);
    }

    /**
     * 把「内层载荷」产物导出到宿主指定位置（`-Dtvbox.payload.out`）：加固壳首次在 unidbg 里
     * 解密出内层 dex、转成 jar 后，宿主侧会自动把它安装成 payload，下次进源改走纯 Java shim，
     * 不必每次重复解密。只在显式注入该属性时生效；失败只打日志，不影响本次加载。
     */
    private static void exportPayloadProduct(File out) {
        String dest = System.getProperty("tvbox.payload.out", "").trim();
        if (dest.isEmpty() || out == null || !out.isFile() || out.length() < 1024L) return;
        try {
            File target = new File(dest);
            File parent = target.getParentFile();
            if (parent != null) parent.mkdirs();
            File tmp = new File(target.getPath() + ".tmp");
            java.io.InputStream in = new java.io.FileInputStream(out);
            java.io.OutputStream os = new java.io.FileOutputStream(tmp);
            try {
                byte[] buf = new byte[65536];
                int n;
                while ((n = in.read(buf)) > 0) os.write(buf, 0, n);
                os.flush();
            } finally {
                try { in.close(); } catch (Throwable ignored) { }
                try { os.close(); } catch (Throwable ignored) { }
            }
            if (tmp.renameTo(target) || target.isFile()) {
                System.err.println("[payload] 内层载荷已导出: " + target.getAbsolutePath()
                        + " (" + target.length() + " B)");
            }
        } catch (Throwable t) {
            System.err.println("[payload] 内层载荷导出失败: " + t);
        }
    }

    private static void finalizeProductBody(File src, File out) {
        try {
            File mark = new File(out.getParentFile(), out.getName() + ".fix.ok");
            if (mark.isFile()) return;
            int copied = copyArchiveResources(src, out);
            int fixed = repairNewSites(src, out);
            System.err.println("[runtime-dex] 产物收尾 " + out.getName()
                    + "：补资源 " + copied + " 项，修 NEW 抽象类 " + (fixed < 0 ? "跳过" : fixed + " 处"));
            try {
                java.nio.file.Files.write(mark.toPath(), "ok".getBytes("UTF-8"));
            } catch (Throwable ignored) {
                // 标记写不出（只读盘）→ 下次再收尾一次，幂等
            }
        } catch (Throwable t) {
            System.err.println("[runtime-dex] 产物收尾失败（继续按产物加载）: " + t);
        }
    }

    /**
     * 把源归档里**非 class 条目**补进转换产物（实测 某第三方源载荷：`assets/libwex_v7.so` /
     * `assets/libwex_v8.so` —— 载荷类 `<clinit>` 会 `getResourceAsStream("assets/libwex_v8.so")`
     * 把 .so 拷出来再 `System.load`，dex2jar 只产 .class ⇒ 不补就恒 null → NPE → 主页全空）。
     * 幂等：产物里已有的条目跳过；有条目缺失时写临时文件再原子替换。
     *
     * @return 补进去的条目数；源不是 zip / 出错 → 0
     */
    private static int copyArchiveResources(File src, File out) {
        if (src == null || out == null || !src.isFile() || !out.isFile()) {
            System.err.println("[runtime-dex] 补资源 skip: src=" + archiveNameOf(src) + " out=" + archiveNameOf(out));
            return 0;
        }
        System.err.println("[runtime-dex] 补资源 enter: src=" + archiveNameOf(src) + " out=" + archiveNameOf(out));
        java.io.File tmp = new java.io.File(out.getParentFile(), out.getName() + ".fix.tmp");
        try {
            java.util.List<String> outNames = new ArrayList<String>();
            java.util.Map<String, byte[]> outData = new java.util.LinkedHashMap<String, byte[]>();
            java.util.List<String> outOrder = new ArrayList<String>();
            java.util.Map<String, byte[]> missing = new java.util.LinkedHashMap<String, byte[]>();
            try (ZipFile zo = new ZipFile(out); ZipFile zs = new ZipFile(src)) {
                Enumeration<? extends ZipEntry> it = zo.entries();
                while (it.hasMoreElements()) {
                    ZipEntry e = it.nextElement();
                    if (e.isDirectory()) continue;
                    outNames.add(e.getName());
                    outOrder.add(e.getName());
                    outData.put(e.getName(), readAllBytes(zo.getInputStream(e)));
                }
                Enumeration<? extends ZipEntry> si = zs.entries();
                while (si.hasMoreElements()) {
                    ZipEntry e = si.nextElement();
                    String n = e.getName();
                    if (e.isDirectory() || n.endsWith(".class") || n.equals("classes.dex")
                            || n.matches("classes\\d+\\.dex") || n.equals("META-INF/MANIFEST.MF")) continue;
                    if (outNames.contains(n)) continue;
                    missing.put(n, readAllBytes(zs.getInputStream(e)));
                }
            }
            if (missing.isEmpty()) return 0;
            try (java.util.zip.ZipOutputStream zos = new java.util.zip.ZipOutputStream(new java.io.FileOutputStream(tmp))) {
                for (String n : outOrder) {
                    zos.putNextEntry(new ZipEntry(n));
                    zos.write(outData.get(n));
                    zos.closeEntry();
                }
                for (java.util.Map.Entry<String, byte[]> e : missing.entrySet()) {
                    zos.putNextEntry(new ZipEntry(e.getKey()));
                    zos.write(e.getValue());
                    zos.closeEntry();
                }
            }
            java.nio.file.Files.move(tmp.toPath(), out.toPath(),
                    java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            return missing.size();
        } catch (Throwable t) {
            try { tmp.delete(); } catch (Throwable ignored) { }
            // 加固壳的容器 zip 元数据不可信（声明尺寸/CRC 与实际不符）→ ZipFile 直接抛错；
            // 这里退回「自己按中央目录 inflate」的宽容读法。
            return copyResourcesLenient(src, out);
        }
    }

    /**
     * 宽容版资源补齐：不信任 zip 元数据（不信 CRC/声明尺寸），只按中央目录的「偏移 + 压缩长度」
     * 自己 inflate。加固壳（如嗷呜）把内层 dex 和 `assets/awenc-v7.so` 一起装在这种容器里，
     * 不补进去 → 蜘蛛 `loadLibrary("awenc-v7")` 恒失败 → MyCrypto 不可用 → 站点配置解不开。
     */
    private static String archiveNameOf(File f) {
        return f == null ? "null" : f.getName() + "(" + f.length() + ")";
    }

    private static int copyResourcesLenient(File src, File out) {
        java.io.RandomAccessFile raf = null;
        java.io.File tmp = new java.io.File(out.getParentFile(), out.getName() + ".fix.tmp");
        try {
            raf = new java.io.RandomAccessFile(src, "r");
            long len = raf.length();
            // —— 中央目录：名字 / CRC / 压缩长度（加固壳的尺寸声明只有这里可信） ——
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
            if (eocd < 0) return 0;
            int total = u16(tail, eocd + 10);
            long cdOff = u32(tail, eocd + 16);
            if (cdOff <= 0 || cdOff >= len) return 0;

            java.util.Map<String, long[]> cd = new java.util.LinkedHashMap<String, long[]>(); // name -> {crc, csize, usize}
            raf.seek(cdOff);
            for (int n = 0; n < total; n++) {
                byte[] h = new byte[46];
                try { raf.readFully(h); } catch (Throwable t) { break; }
                if (u32(h, 0) != 0x02014b50L) break;
                long crc = u32(h, 16);
                long csize = u32(h, 20);
                long usize = u32(h, 24);
                int nameLen = u16(h, 28);
                int extraLen = u16(h, 30);
                int commentLen = u16(h, 32);
                byte[] nb = new byte[nameLen];
                try { raf.readFully(nb); } catch (Throwable t) { break; }
                String name = new String(nb, "UTF-8");
                cd.put(name, new long[]{crc, csize, usize});
                raf.seek(raf.getFilePointer() + extraLen + commentLen);
            }

            // —— 本地头：加固壳的中央目录「本地偏移」被整体平移过（实测 -1202），
            //    但文件里仍有合法的 `PK\x03\x04` 本地头 → 全文件扫描，按条目名对回去。 ——
            java.util.Map<String, Long> dataOff = new java.util.LinkedHashMap<String, Long>();
            byte[] block = new byte[1 << 20];
            long scan = 0;
            while (scan < len) {
                int want = (int) Math.min(block.length, len - scan);
                raf.seek(scan);
                raf.readFully(block, 0, want);
                for (int i = 0; i + 30 <= want; i++) {
                    if ((block[i] & 0xff) != 0x50 || (block[i + 1] & 0xff) != 0x4b
                            || (block[i + 2] & 0xff) != 0x03 || (block[i + 3] & 0xff) != 0x04) {
                        continue;
                    }
                    int nlen = u16(block, i + 26);
                    int elen = u16(block, i + 28);
                    if (i + 30 + nlen > want || nlen <= 0 || nlen > 512) continue;
                    String name = new String(block, i + 30, nlen, "UTF-8");
                    if (!cd.containsKey(name)) continue;
                    dataOff.put(name, scan + i + 30 + nlen + elen);
                }
                if (want < block.length) break;
                scan += want - 4096L; // 保留跨界重叠
                if (scan < 0) break;
            }

            java.util.Map<String, byte[]> missing = new java.util.LinkedHashMap<String, byte[]>();
            for (java.util.Map.Entry<String, long[]> e : cd.entrySet()) {
                String name = e.getKey();
                if (name.endsWith("/") || name.endsWith(".class") || name.equals("classes.dex")
                        || name.matches("classes\\d+\\.dex") || name.equals("META-INF/MANIFEST.MF")) {
                    continue;
                }
                Long dsObj = dataOff.get(name);
                if (dsObj == null) continue;
                long ds = dsObj;
                long csize = e.getValue()[1];
                long crc = e.getValue()[0];
                if (csize <= 0 || csize > (64L << 20) || ds + csize > len) continue;
                byte[] comp = new byte[(int) csize];
                raf.seek(ds);
                raf.readFully(comp);
                byte[] plain;
                java.util.zip.Inflater inf = new java.util.zip.Inflater(true);
                try {
                    inf.setInput(comp);
                    java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream(comp.length * 2);
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
                if (plain.length == 0) continue;
                if (crc != 0) {
                    java.util.zip.CRC32 c = new java.util.zip.CRC32();
                    c.update(plain);
                    if (c.getValue() != crc) continue; // CRC 不符 → 不采信
                }
                missing.put(name, plain);
            }
            if (missing.isEmpty()) return 0;

            java.util.List<String> outOrder = new java.util.ArrayList<String>();
            java.util.Map<String, byte[]> outData = new java.util.LinkedHashMap<String, byte[]>();
            try (java.util.zip.ZipFile zo = new java.util.zip.ZipFile(out)) {
                java.util.Enumeration<? extends java.util.zip.ZipEntry> it = zo.entries();
                while (it.hasMoreElements()) {
                    java.util.zip.ZipEntry e = it.nextElement();
                    if (e.isDirectory()) continue;
                    outOrder.add(e.getName());
                    outData.put(e.getName(), readAllBytes(zo.getInputStream(e)));
                }
            }
            int added = 0;
            try (java.util.zip.ZipOutputStream zos = new java.util.zip.ZipOutputStream(new java.io.FileOutputStream(tmp))) {
                for (String n : outOrder) {
                    zos.putNextEntry(new java.util.zip.ZipEntry(n));
                    zos.write(outData.get(n));
                    zos.closeEntry();
                }
                for (java.util.Map.Entry<String, byte[]> e : missing.entrySet()) {
                    if (outData.containsKey(e.getKey())) continue;
                    zos.putNextEntry(new java.util.zip.ZipEntry(e.getKey()));
                    zos.write(e.getValue());
                    zos.closeEntry();
                    added++;
                }
            }
            java.nio.file.Files.move(tmp.toPath(), out.toPath(),
                    java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            System.err.println("[runtime-dex] 宽容补资源：从加固壳容器恢复 " + added + " 个资源 " + missing.keySet());
            return added;
        } catch (Throwable t) {
            try { tmp.delete(); } catch (Throwable ignored) { }
            System.err.println("[runtime-dex] 宽容补资源失败: " + t);
            return 0;
        } finally {
            if (raf != null) {
                try { raf.close(); } catch (Throwable ignored) { }
            }
        }
    }

    private static int repairNewSites(File dex, File out) {
        try {
            Class<?> c = Class.forName("com.winbox.nativebridge.DexJarRepair");
            Object r = c.getMethod("repair", File.class, File.class, String.class, String.class)
                    .invoke(null, dex, out,
                            System.getProperty("tvbox.d2j.java", "java"),
                            System.getProperty("tvbox.d2j.cp", ""));
            return r instanceof Integer ? ((Integer) r).intValue() : 0;
        } catch (Throwable t) {
            return -1;
        }
    }

    /** 读完整流（小文件用；assets 级别几 MB 可接受） */
    private static byte[] readAllBytes(InputStream in) throws IOException {
        try {
            java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream();
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) bos.write(buf, 0, n);
            return bos.toByteArray();
        } finally {
            in.close();
        }
    }

    /** 产物是否「写完整」：能作为 zip 打开且至少有一个 .class（写一半被杀的文件没有中央目录）。 */
    private static boolean isCompleteProduct(File f) {
        if (f == null || !f.isFile() || f.length() == 0) return false;
        try (ZipFile zf = new ZipFile(f)) {
            Enumeration<? extends ZipEntry> it = zf.entries();
            while (it.hasMoreElements()) {
                if (it.nextElement().getName().endsWith(".class")) return true;
            }
            return false;
        } catch (Throwable t) {
            return false;
        }
    }

    /** 把已经写完的 part 收编为正式产物（尽力而为；失败只当作没赶上，下次照常重转）。 */
    private static boolean adoptProduct(File part, File out) {
        try {
            if (out.isFile() && !out.delete()) return false;
            if (part.renameTo(out)) return true;
            return out.isFile() && out.length() > 0;
        } catch (Throwable t) {
            return false;
        }
    }

    /** 流式 md5（dex 可能几十 MB，不能整读进内存）。失败返回 null。 */
    private static String md5Of(File f) {
        try (InputStream in = new FileInputStream(f)) {
            MessageDigest md = MessageDigest.getInstance("MD5");
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) md.update(buf, 0, n);
            StringBuilder sb = new StringBuilder();
            for (byte b : md.digest()) sb.append(String.format("%02x", b));
            return sb.toString();
        } catch (Throwable t) {
            return null;
        }
    }

    @Override
    protected Class<?> findClass(String name) throws ClassNotFoundException {
        if (delegate != null) {
            try {
                return delegate.loadClass(name);
            } catch (ClassNotFoundException ignored) {
                // 落到下面的统一报错
            }
        }
        if (!unsupported.isEmpty()) {
            throw new ClassNotFoundException(name
                    + "（桌面版无法加载：该路径含 " + unsupported
                    + "；裸 dex 需宿主注入 tvbox.d2j.* 才能运行时转换）");
        }
        throw new ClassNotFoundException(name);
    }

    /** 兜底：某些壳只用 loadClass，不走 findClass（本类直接委托）。 */
    @Override
    public Class<?> loadClass(String name) throws ClassNotFoundException {
        // VBox 本地补丁（2026-11-23）：child-first 模式。
        //   加固壳的内层 dex 与外壳 jar 常有同名类（尤其混淆后的 merge.* 工具类），
        //   Android 上外壳不在 parent 里所以没有冲突；桌面版外壳 jar 在父加载器上，
        //   parent-first 会让内层代码错用外壳的旧版本 → NoSuchFieldError（实测 aowu 的
        //   AwInit.init 崩在 `o0OoO0oO0oOoO0O0oO`）。
        //   开启后先查自己；`tvbox.loader.parentFirst` 里的类仍走父加载器（影子类必须如此）。
        if (Boolean.parseBoolean(System.getProperty("tvbox.loader.childFirst", "false"))) {
            for (String pf : System.getProperty("tvbox.loader.parentFirst", "").split(",")) {
                if (!pf.trim().isEmpty() && pf.trim().equals(name)) {
                    return super.loadClass(name);
                }
            }
            try {
                return findClass(name);
            } catch (ClassNotFoundException ignored) {
                // 自己也没有 → 交回父加载器
            }
            return super.loadClass(name);
        }
        Class<?> c = null;
        try {
            c = super.loadClass(name);
        } catch (ClassNotFoundException ignored) {
            // 父加载器没有 → 尝试自己
        }
        if (c != null) return c;
        return findClass(name);
    }

    public String getDexPath() {
        return dexPath;
    }

    /** 供诊断：返回本加载器无法处理的路径条目。 */
    public List<String> getUnsupportedPaths() {
        return new ArrayList<String>(unsupported);
    }

    /** 供诊断：是否具备真正可用的加载能力。 */
    public boolean isFunctional() {
        return delegate != null;
    }

    /** 安卓 API 上有这个方法，加固代码常调用它做大包路径判断。 */
    public String findLibrary(String name) {
        if (librarySearchPath == null || name == null) return null;
        for (String dir : librarySearchPath.split(File.pathSeparator)) {
            if (dir == null || dir.trim().isEmpty()) continue;
            File f = new File(dir.trim(), System.mapLibraryName(name));
            if (f.exists()) return f.getAbsolutePath();
        }
        return null;
    }

    @Override
    public String toString() {
        return getClass().getName() + "[dexPath=" + dexPath
                + ", functional=" + isFunctional() + "]";
    }
}
