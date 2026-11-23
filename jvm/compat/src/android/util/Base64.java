/*
 * VBox JVM 兼容层自研实现（不属于 jvm/upstream 的 MIT 上游代码）。
 * android.util.Base64 —— TVBox 蜘蛛常用：Base64.encodeToString / decode。
 */
package android.util;

import java.nio.charset.StandardCharsets;

public final class Base64 {
    public static final int DEFAULT = 0;
    public static final int NO_PADDING = 1;
    public static final int NO_WRAP = 2;
    public static final int CRLF = 4;
    public static final int URL_SAFE = 8;
    public static final int NO_CLOSE = 16;

    private Base64() {}

    public static byte[] decode(String str, int flags) {
        if (str == null) return new byte[0];
        String s = stripWhitespace(str);
        try {
            if ((flags & URL_SAFE) != 0) return java.util.Base64.getUrlDecoder().decode(pad(s));
            return java.util.Base64.getDecoder().decode(pad(s));
        } catch (IllegalArgumentException e) {
            return new byte[0];
        }
    }

    public static byte[] decode(byte[] input, int flags) {
        return decode(new String(input, StandardCharsets.US_ASCII), flags);
    }

    public static byte[] decode(byte[] input, int offset, int len, int flags) {
        return decode(new String(input, offset, len, StandardCharsets.US_ASCII), flags);
    }

    public static String encodeToString(byte[] input, int flags) {
        if (input == null) return "";
        String s;
        if ((flags & URL_SAFE) != 0) s = java.util.Base64.getUrlEncoder().encodeToString(input);
        else s = java.util.Base64.getEncoder().encodeToString(input);
        if ((flags & NO_PADDING) != 0) s = s.replace("=", "");
        if ((flags & NO_WRAP) == 0 && (flags & CRLF) != 0 && s.length() > 76) {
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < s.length(); i += 76) sb.append(s, i, Math.min(s.length(), i + 76)).append("\r\n");
            s = sb.toString();
        }
        return s;
    }

    public static byte[] encode(byte[] input, int flags) {
        return encodeToString(input, flags).getBytes(StandardCharsets.US_ASCII);
    }

    public static byte[] encode(byte[] input, int offset, int len, int flags) {
        byte[] slice = new byte[len];
        System.arraycopy(input, offset, slice, 0, len);
        return encode(slice, flags);
    }

    private static String stripWhitespace(String s) {
        StringBuilder sb = new StringBuilder(s.length());
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            if (c != '\n' && c != '\r' && c != ' ' && c != '\t') sb.append(c);
        }
        return sb.toString();
    }

    private static String pad(String s) {
        int rem = s.length() % 4;
        if (rem == 0) return s;
        StringBuilder sb = new StringBuilder(s);
        for (int i = 0; i < 4 - rem; i++) sb.append('=');
        return sb.toString();
    }
}
