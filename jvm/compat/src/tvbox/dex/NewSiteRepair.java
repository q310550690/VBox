/*
 * VBox JVM 兼容层自研实现（AGPL-3.0-only）
 *
 * 修 dex2jar 对 R8「去掉构造器」类产物的误译：
 *   R8 会把「只被 new 但构造器被优化掉」的类标成 abstract 并删除 <init>，
 *   dex2jar 忠实翻出 `new <abstract>` + `invokespecial <init>`，JVM 直接
 *   抛 InstantiationError（实测加固壳内层 dex）。
 *
 * 这里在**转换产物**上做一次性修补（纯 ASM，不依赖 smali/baksmali）：
 *   1) 读全部 class，收集 `NEW` 目标与每个类的构造器集合；
 *   2) 对被 new 却没有 <init> 的类：去掉 ACC_ABSTRACT、补一个 `<init>()V`
 *      （先调父类 <init>()V，父类缺则调 Object.<init>()V）；
 *   3) 重写 jar（ZipInputStream/ZipOutputStream，不碰 zipfs）。
 */
package tvbox.dex;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.ClassNode;
import org.objectweb.asm.tree.MethodNode;
import org.objectweb.asm.tree.TypeInsnNode;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;

public final class NewSiteRepair {

    private NewSiteRepair() {
    }

    /** @return 修好的类数；0 表示没有需要修的 */
    public static int repair(File jar) {
        try {
            Map<String, byte[]> entries = new LinkedHashMap<String, byte[]>();
            List<String> order = new ArrayList<String>();
            try (ZipFile zf = new ZipFile(jar)) {
                java.util.Enumeration<? extends ZipEntry> it = zf.entries();
                while (it.hasMoreElements()) {
                    ZipEntry e = it.nextElement();
                    if (e.isDirectory()) {
                        continue;
                    }
                    try (InputStream in = zf.getInputStream(e)) {
                        entries.put(e.getName(), readAll(in));
                    }
                    order.add(e.getName());
                }
            }
            Map<String, ClassNode> nodes = new HashMap<String, ClassNode>();
            Set<String> newTargets = new HashSet<String>();
            for (Map.Entry<String, byte[]> e : entries.entrySet()) {
                if (!e.getKey().endsWith(".class")) {
                    continue;
                }
                ClassNode cn = new ClassNode();
                new ClassReader(e.getValue()).accept(cn, 0);
                nodes.put(cn.name, cn);
                for (MethodNode m : cn.methods) {
                    for (AbstractInsnNode insn : m.instructions.toArray()) {
                        if (insn.getOpcode() == Opcodes.NEW && insn instanceof TypeInsnNode) {
                            newTargets.add(((TypeInsnNode) insn).desc);
                        }
                    }
                }
            }
            // 默认 both：去 abstract 走「原字节 access_flags 补丁」（不重写代码布局），
            // 缺构造器才走 ASM 往返。历史教训（2026-11-23，嗷呜 awdatabase.db）：
            // 早期用 ASM 往返去掉 abstract，会让加固壳混淆类在解释器里跑坏
            // （EXCEPTION_ACCESS_VIOLATION；d2j 原始产物不崩）。
            String mode = System.getProperty("tvbox.newrepair.mode", "both");
            if (mode == null || mode.isEmpty()) {
                mode = "both";
            }
            mode = mode.toLowerCase();
            boolean doAbstract = !"ctor".equals(mode) && !"none".equals(mode);
            boolean doCtor = !"abstract".equals(mode) && !"none".equals(mode);
            System.err.println("[new-fix] mode=" + mode + "（doAbstract=" + doAbstract + " doCtor=" + doCtor + "）");
            int fixed = 0;
            for (Map.Entry<String, ClassNode> e : nodes.entrySet()) {
                ClassNode cn = e.getValue();
                if (!newTargets.contains(cn.name)) {
                    continue;
                }
                if ((cn.access & Opcodes.ACC_INTERFACE) != 0) {
                    continue;
                }
                boolean hasInit = false;
                for (MethodNode m : cn.methods) {
                    if ("<init>".equals(m.name)) {
                        hasInit = true;
                        break;
                    }
                }
                boolean needCtor = doCtor && !hasInit;
                boolean needStrip = doAbstract && (cn.access & Opcodes.ACC_ABSTRACT) != 0;
                if (!needCtor && !needStrip) {
                    continue;
                }
                byte[] original = entries.get(cn.name + ".class");
                byte[] next = original;
                if (needStrip && original != null) {
                    // R8 常见形态：类被标 abstract 但保留了构造器（实例化点仍 new + invokespecial<init>）。
                    // 这里**只清原字节 access_flags 里的 ACC_ABSTRACT 位**（偏移不固定，按常量池定位），
                    // 不做 ASM 往返重写 —— 实测往返会让加固壳的混淆类在解释器里跑成非法状态
                    // （`String.trim` 收非法引用 → EXCEPTION_ACCESS_VIOLATION），而原字节补丁不影响布局。
                    byte[] patched = stripAbstractInPlace(original);
                    if (patched != null) {
                        next = patched;
                        System.err.println("[new-fix] 去 abstract（原字节补丁）: " + cn.name);
                    }
                }
                if (needCtor && next != null) {
                    // 构造器被裁剪的形态：补一个 ()V（父类缺则找 Object）。这里必须走 ASM 往返，
                    // 但只涉及 4 个类，实测稳定（不触发上面的崩溃）。
                    ClassNode fresh = new ClassNode();
                    new ClassReader(next).accept(fresh, 0);
                    String superName = fresh.superName == null ? "java/lang/Object" : fresh.superName;
                    MethodVisitor mv = fresh.visitMethod(Opcodes.ACC_PUBLIC, "<init>", "()V", null, null);
                    mv.visitCode();
                    mv.visitVarInsn(Opcodes.ALOAD, 0);
                    mv.visitMethodInsn(Opcodes.INVOKESPECIAL, superName, "<init>", "()V", false);
                    mv.visitInsn(Opcodes.RETURN);
                    mv.visitMaxs(1, 1);
                    mv.visitEnd();
                    ClassWriter cw = new ClassWriter(new ClassReader(next), 0);
                    fresh.accept(cw);
                    next = cw.toByteArray();
                    System.err.println("[new-fix] 补 <init>()V: " + cn.name);
                }
                if (next == null) {
                    continue;
                }
                entries.put(cn.name + ".class", next);
                fixed++;
            }
            if (fixed == 0) {
                return 0;
            }
            File tmp = new File(jar.getParentFile(), jar.getName() + ".newfix.tmp");
            try (ZipOutputStream zos = new ZipOutputStream(new FileOutputStream(tmp))) {
                for (String n : order) {
                    byte[] data = entries.get(n);
                    if (data == null) {
                        continue;
                    }
                    zos.putNextEntry(new ZipEntry(n));
                    zos.write(data);
                    zos.closeEntry();
                }
            }
            java.nio.file.Files.move(tmp.toPath(), jar.toPath(),
                    java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            System.err.println("[new-fix] 修补 NEW 抽象类 " + fixed + " 个（补 <init> + 去 abstract）");
            return fixed;
        } catch (Throwable t) {
            System.err.println("[new-fix] 修补失败（保持原产物）: " + t);
            return 0;
        }
    }

    /**
     * 只把 class 文件 access_flags 的 ACC_ABSTRACT 位清掉，其余字节保持不变。
     * 偏移不固定，需按常量池逐项跳过定位；解析失败返回 null（调用方保留原字节）。
     */
    private static byte[] stripAbstractInPlace(byte[] b) {
        try {
            if (b == null || b.length < 12) {
                return null;
            }
            int cpCount = u2(b, 8);
            int pos = 10;
            for (int i = 1; i < cpCount; i++) {
                int tag = b[pos] & 0xff;
                pos += 1;
                switch (tag) {
                    case 1: // Utf8
                        pos += 2 + u2(b, pos);
                        break;
                    case 3: // Integer
                    case 4: // Float
                        pos += 4;
                        break;
                    case 5: // Long
                    case 6: // Double
                        pos += 8;
                        i++;
                        break;
                    case 7: // Class
                    case 8: // String
                    case 16: // MethodType
                    case 19: // Module
                    case 20: // Package
                        pos += 2;
                        break;
                    case 15: // MethodHandle
                        pos += 3;
                        break;
                    case 9: // Fieldref
                    case 10: // Methodref
                    case 11: // InterfaceMethodref
                    case 12: // NameAndType
                    case 17: // Dynamic
                    case 18: // InvokeDynamic
                        pos += 4;
                        break;
                    default:
                        return null;
                }
            }
            if (pos + 2 > b.length) {
                return null;
            }
            int flags = u2(b, pos);
            if ((flags & Opcodes.ACC_ABSTRACT) == 0) {
                return b;
            }
            byte[] out = b.clone();
            out[pos] = (byte) ((flags & ~Opcodes.ACC_ABSTRACT) >> 8);
            out[pos + 1] = (byte) ((flags & ~Opcodes.ACC_ABSTRACT) & 0xff);
            return out;
        } catch (RuntimeException e) {
            return null;
        }
    }

    private static int u2(byte[] b, int off) {
        return ((b[off] & 0xff) << 8) | (b[off + 1] & 0xff);
    }

    private static byte[] readAll(InputStream in) throws Exception {
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        byte[] buf = new byte[8192];
        int n;
        while ((n = in.read(buf)) > 0) {
            bos.write(buf, 0, n);
        }
        return bos.toByteArray();
    }
}
