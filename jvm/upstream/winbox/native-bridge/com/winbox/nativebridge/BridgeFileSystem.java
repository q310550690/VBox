package com.winbox.nativebridge;

import com.github.unidbg.Emulator;
import com.github.unidbg.file.FileResult;
import com.github.unidbg.file.linux.AndroidFileIO;
import com.github.unidbg.file.linux.LinuxFileSystem;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

/** Native libraries may receive desktop cache paths from ordinary Java code.
 * Map only the explicitly configured spider sandbox, never arbitrary host paths.
 * POSIX paths continue to use the emulator's own rootfs. */
public final class BridgeFileSystem extends LinuxFileSystem {
    private LinuxFileSystem hostFiles;
    private final File sandbox;
    private final File rootfsHostDir;
    private static final String DENIED = "\u0000";

    public BridgeFileSystem(Emulator<AndroidFileIO> emulator, File root) {
        super(emulator, root);
        String configured = System.getProperty("tvbox.spiderCacheDir");
        sandbox = configured == null || configured.isEmpty() ? null : new File(configured);
        rootfsHostDir = root == null ? null : root.getAbsoluteFile();
    }

    private String hostPath(String pathname) {
        String normalized = pathname.replace('\\', '/');
        if (!normalized.matches("^/?[a-zA-Z]:/.*") && !normalized.startsWith("//")) return null;
        if (File.separatorChar != '\\' || sandbox == null || normalized.startsWith("//")) return DENIED;
        if (normalized.charAt(0) == '/') normalized = normalized.substring(1);
        try {
            Path root = sandbox.toPath().toRealPath();
            Path requested = new File(normalized).toPath().toAbsolutePath().normalize();
            if (!requested.startsWith(sandbox.toPath().toAbsolutePath().normalize())) return DENIED;
            // Resolve the nearest existing ancestor too: a junction/symlink in
            // the sandbox must not turn a permitted cache path into host access.
            Path ancestor = requested;
            while (ancestor != null && !Files.exists(ancestor, java.nio.file.LinkOption.NOFOLLOW_LINKS)) ancestor = ancestor.getParent();
            if (ancestor == null) return DENIED;
            Path physical = ancestor.toRealPath().resolve(ancestor.relativize(requested)).normalize();
            if (!physical.startsWith(root)) return DENIED;
            String relative = root.relativize(physical).toString().replace('\\', '/');
            return relative.isEmpty() ? "." : relative;
        } catch (IOException | RuntimeException e) { return DENIED; }
    }

    /** Desktop drive-letter paths that point inside the emulator rootfs are the
     * guest's own files seen through the host view (e.g. Application.getCacheDir()
     * returns a host path). Map them back to the guest POSIX path so the unidbg
     * rootfs handles the IO, instead of rejecting them as host paths. */
    private String guestUnderRootfs(String pathname) {
        if (rootfsHostDir == null || pathname == null) return null;
        String normalized = pathname.replace('\\', '/');
        if (!normalized.matches("^/?[a-zA-Z]:/.*")) return null;
        if (normalized.charAt(0) == '/') normalized = normalized.substring(1);
        String root = rootfsHostDir.getAbsolutePath().replace('\\', '/');
        while (root.endsWith("/")) root = root.substring(0, root.length() - 1);
        if (normalized.length() <= root.length() + 1) return null;
        if (!normalized.regionMatches(true, 0, root, 0, root.length())) return null;
        String rest = normalized.substring(root.length());
        if (!rest.startsWith("/") || rest.length() <= 1) return null;
        return rest;
    }

    private LinuxFileSystem hostFiles() {
        if (hostFiles == null) hostFiles = new LinuxFileSystem(emulator, sandbox);
        return hostFiles;
    }

    @Override public FileResult<AndroidFileIO> open(String pathname, int flags) {
        String guest = guestUnderRootfs(pathname);
        if (guest != null) return super.open(guest, flags);
        String mapped = hostPath(pathname);
        if (mapped == null) return super.open(pathname, flags);
        if (DENIED.equals(mapped)) return FileResult.failed(13);
        return hostFiles().open(mapped, flags);
    }

    @Override public boolean mkdir(String pathname, int mode) {
        String guest = guestUnderRootfs(pathname);
        if (guest != null) return super.mkdir(guest, mode);
        String mapped = hostPath(pathname);
        if (mapped == null) return super.mkdir(pathname, mode);
        if (DENIED.equals(mapped)) return false;
        return hostFiles().mkdir(mapped, mode);
    }

    @Override public void unlink(String pathname) {
        String guest = guestUnderRootfs(pathname);
        if (guest != null) { super.unlink(guest); return; }
        String mapped = hostPath(pathname);
        if (mapped == null) { super.unlink(pathname); return; }
        remove(mapped, false);
    }

    @Override public void rmdir(String pathname) {
        String guest = guestUnderRootfs(pathname);
        if (guest != null) { super.rmdir(guest); return; }
        String mapped = hostPath(pathname);
        if (mapped == null) { super.rmdir(pathname); return; }
        remove(mapped, true);
    }

    private void remove(String mapped, boolean directory) {
        // No recursive deletes, and never delete the sandbox itself.
        if (DENIED.equals(mapped) || ".".equals(mapped)) throw new SecurityException("Native path outside spider sandbox");
        Path target = new File(sandbox, mapped).toPath();
        if (Files.isDirectory(target) != directory) throw new IllegalStateException("Native removal has wrong file type");
        try { Files.delete(target); }
        catch (IOException e) { throw new IllegalStateException("Native removal failed", e); }
    }

    @Override public int rename(String oldPath, String newPath) {
        String oldGuest = guestUnderRootfs(oldPath), newGuest = guestUnderRootfs(newPath);
        if (oldGuest != null && newGuest != null) return super.rename(oldGuest, newGuest);
        if (oldGuest != null || newGuest != null) return -18; // EXDEV
        String oldMapped = hostPath(oldPath), newMapped = hostPath(newPath);
        if (oldMapped == null && newMapped == null) return super.rename(oldPath, newPath);
        if (oldMapped == null || newMapped == null) return -18; // EXDEV
        if (DENIED.equals(oldMapped) || DENIED.equals(newMapped) || ".".equals(oldMapped) || ".".equals(newMapped)) return -13;
        try {
            Files.move(new File(sandbox, oldMapped).toPath(), new File(sandbox, newMapped).toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            return 0;
        } catch (java.nio.file.NoSuchFileException e) { return -2; }
        catch (IOException e) { return -5; }
    }
}
