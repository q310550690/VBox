/** 把后端的原始错误翻译成用户能理解的能力边界说明。
 *
 *  - DRM（FairPlay `skd://`、非 identity `KEYFORMAT`、Widevine 等）：无法解密，
 *    播放与离线保存都不支持；
 *  - SAMPLE-AES + fMP4（`#EXT-X-MAP`）：当前离线解密链路不支持；
 *  - 其他错误原样返回，避免吞掉解析失败等有效信息。
 */
export function explainMediaError(
  raw: string | null | undefined,
  context: 'play' | 'download' = 'play',
): string {
  const text = (raw ?? '').trim()
  if (!text) return ''
  if (/skd:\/\/|fairplay|widevine|keyformat|drm/i.test(text)) {
    return context === 'download'
      ? '该线路使用 DRM 加密（FairPlay/Widevine），当前版本不支持离线保存；可换线路或继续在线观看。'
      : '该线路使用 DRM 加密（FairPlay/Widevine），当前版本不支持播放；请切换线路或解析源。'
  }
  if (/sample-aes\s*\+\s*fmp4|ext-x-map/i.test(text)) {
    return context === 'download'
      ? '该线路是 SAMPLE-AES + fMP4，暂不支持离线保存；可换线路。'
      : '该线路是 SAMPLE-AES + fMP4，当前版本暂不支持播放；请换线路。'
  }
  return text
}
