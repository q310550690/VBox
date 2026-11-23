/*
 * VBox JVM 兼容层自研实现（AGPL-3.0-only）
 *
 * 子进程入口：dex/zip → jar，只用普通 FileOutputStream（不碰 zipfs）。
 * 由 `dalvik.system.BaseDexClassLoader` 在运行时拉起：
 *   java -cp "<dex2jar jars>;<stubs.jar>" tvbox.dex.ConvertMain <src> <out.jar>
 * 成功时 stdout 打印 `CLASSES=<n>`，失败非 0 退出。
 */
package tvbox.dex;

import java.io.File;

public final class ConvertMain {

    public static void main(String[] args) {
        if (args.length < 2) {
            System.err.println("用法: ConvertMain <src> <out.jar>");
            System.exit(2);
        }
        File src = new File(args[0]);
        File out = new File(args[1]);
        File dex = null;
        try {
            dex = DexStreamConverter.extractDexIfZip(src, new File(out.getParentFile(), out.getName() + ".dex"));
            if (dex == null) {
                System.err.println("[convert] 源不是可识别的 dex/zip: " + src);
                System.exit(3);
            }
            int n = DexStreamConverter.convert(dex, out);
            int failed = DexStreamConverter.lastFailed();
            int fixed = NewSiteRepair.repair(out);
            // 加固壳要在被加载的 jar 里找 assets/*.so 起 native 桥：转换只产出 class，资源要单独补。
            int res = DexStreamConverter.copyResources(src, out);
            System.out.println("CLASSES=" + n + " NEWFIX=" + fixed + " FAILED=" + failed + " RES=" + res);
            if (n == 0) {
                System.exit(4);
            }
            // 有失败类就返回 3：调用方（dex.rs）会回退到官方 Dex2jarCmd 重转，
            // 那个实现会吞掉坏类继续翻译，产物更完整。
            // 注意：即使有少数类失败也返回 0 —— 调用方（dex.rs）用 dex 头的 class
            // 总数做完整性闸门（<90% 才回退官方 Dex2jarCmd），避免把 1355/1356 这种
            // 可用产物丢掉。
            System.exit(0);
        } catch (Throwable t) {
            t.printStackTrace();
            System.exit(5);
        }
    }
}
