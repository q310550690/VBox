// VBox 自研补件（AGPL-3.0-only）：Win-box 的 BridgeARM32Emulator 引用了它自带的
// unidbg 补丁里的 SocketReadiness，但上游仓库里没有这个类的源码。这里用最保守的
// 语义补上：可用字节数 / 可写性 / 代人（epoch）都用 socket 流的就绪状态近似。
package com.github.unidbg.linux.file;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

public final class SocketReadiness {
    private SocketReadiness() {}

    public static boolean readable(TcpSocket socket) {
        try {
            InputStream in = socket.inputStream;
            return in != null && in.available() > 0;
        } catch (IOException e) {
            return false;
        }
    }

    public static boolean writable(TcpSocket socket) {
        return socket.outputStream != null;
    }

    /** 用「是否有未读字节」当作读纪元，足够驱动边缘触发的 epoll 语义。 */
    public static long epoch(TcpSocket socket, boolean read) {
        if (read) {
            try {
                InputStream in = socket.inputStream;
                return in == null ? 0 : in.available();
            } catch (IOException e) {
                return 0;
            }
        }
        OutputStream out = socket.outputStream;
        return out == null ? 0 : 1;
    }
}
