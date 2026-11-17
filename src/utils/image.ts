/**
 * 图片地址处理。
 *
 * 豆瓣封面（img*.doubanio.com）直连需要 `Referer: https://movie.douban.com/`，
 * 在 WebView 里直接 <img src> 会返回 418。桌面端由 Rust 本机服务提供
 * `GET /img?url=<图片地址>` 代理并把前缀作为 `imageProxy` 返回；这里负责拼接。
 * 浏览器预览没有本机服务（imageProxy 为空），演示数据用可直接访问的图片地址。
 */
export function proxyImage(
  url: string | null | undefined,
  proxy: string | null | undefined,
): string {
  const src = (url ?? '').trim()
  if (!src) return ''
  // 非 HTTP(S)（data: / blob: / clan: / 相对路径）不做代理
  if (!/^https?:\/\//i.test(src)) return src
  const prefix = (proxy ?? '').trim()
  if (!prefix) return src
  // 已经是我们本机代理地址时不重复拼接
  if (src.startsWith(prefix)) return src
  return prefix + encodeURIComponent(src)
}
