/**
 * Solar Linear 图标集合（https://solar-icons.vercel.app/）
 *
 * 图标源文件在 `src/assets/solar-linear/`（1400+ 个），本项目**只**把实际用到的
 * 那些以 `?raw` 方式内联进来：既能保留 `currentColor`（跟随文字颜色），
 * 又不会让整目录进 bundle。
 *
 * 新增图标：从 `src/assets/solar-linear/` 挑一个文件，加一行 import + 映射。
 * key 规则 = 文件名去掉 `solar--` 前缀和 `-linear.svg` 后缀，
 * 例如 `solar--video-frame-linear.svg` → `video-frame`。
 */
import add from '@/assets/solar-linear/solar--add-linear.svg?raw'
import altArrowDown from '@/assets/solar-linear/solar--alt-arrow-down-linear.svg?raw'
import altArrowLeft from '@/assets/solar-linear/solar--alt-arrow-left-linear.svg?raw'
import altArrowRight from '@/assets/solar-linear/solar--alt-arrow-right-linear.svg?raw'
import altArrowUp from '@/assets/solar-linear/solar--alt-arrow-up-linear.svg?raw'
import arrowDown from '@/assets/solar-linear/solar--arrow-down-linear.svg?raw'
import arrowLeft from '@/assets/solar-linear/solar--arrow-left-linear.svg?raw'
import arrowRight from '@/assets/solar-linear/solar--arrow-right-linear.svg?raw'
import arrowUp from '@/assets/solar-linear/solar--arrow-up-linear.svg?raw'
import sortVertical from '@/assets/solar-linear/solar--sort-vertical-linear.svg?raw'
import box from '@/assets/solar-linear/solar--box-linear.svg?raw'
import chatRoundLine from '@/assets/solar-linear/solar--chat-round-line-linear.svg?raw'
import checkCircle from '@/assets/solar-linear/solar--check-circle-linear.svg?raw'
import clapperboard from '@/assets/solar-linear/solar--clapperboard-linear.svg?raw'
import cloudStorage from '@/assets/solar-linear/solar--cloud-storage-linear.svg?raw'
import cloudUpload from '@/assets/solar-linear/solar--cloud-upload-linear.svg?raw'
import clockCircle from '@/assets/solar-linear/solar--clock-circle-linear.svg?raw'
import close from '@/assets/solar-linear/solar--close-linear.svg?raw'
import closeCircle from '@/assets/solar-linear/solar--close-circle-linear.svg?raw'
import copy from '@/assets/solar-linear/solar--copy-linear.svg?raw'
import dangerTriangle from '@/assets/solar-linear/solar--danger-triangle-linear.svg?raw'
import download from '@/assets/solar-linear/solar--download-linear.svg?raw'
import downloadMinimalistic from '@/assets/solar-linear/solar--download-minimalistic-linear.svg?raw'
import filePen from '@/assets/solar-linear/solar--file-pen-linear.svg?raw'
import flag from '@/assets/solar-linear/solar--flag-linear.svg?raw'
import flag2 from '@/assets/solar-linear/solar--flag-2-linear.svg?raw'
import folderOpen from '@/assets/solar-linear/solar--folder-open-linear.svg?raw'
import fullScreen from '@/assets/solar-linear/solar--full-screen-linear.svg?raw'
import headphonesRound from '@/assets/solar-linear/solar--headphones-round-linear.svg?raw'
import highQuality from '@/assets/solar-linear/solar--high-quality-linear.svg?raw'
import infoCircle from '@/assets/solar-linear/solar--info-circle-linear.svg?raw'
import lock from '@/assets/solar-linear/solar--lock-linear.svg?raw'
import magnifier from '@/assets/solar-linear/solar--magnifier-linear.svg?raw'
import maximizeSquare from '@/assets/solar-linear/solar--maximize-square-linear.svg?raw'
import menuDots from '@/assets/solar-linear/solar--menu-dots-linear.svg?raw'
import minimizeSquare from '@/assets/solar-linear/solar--minimize-square-linear.svg?raw'
import minus from '@/assets/solar-linear/solar--minus-linear.svg?raw'
import monitor from '@/assets/solar-linear/solar--monitor-linear.svg?raw'
import moon from '@/assets/solar-linear/solar--moon-linear.svg?raw'
import musicNotes from '@/assets/solar-linear/solar--music-notes-linear.svg?raw'
import muted from '@/assets/solar-linear/solar--muted-linear.svg?raw'
import pause from '@/assets/solar-linear/solar--pause-linear.svg?raw'
import pip from '@/assets/solar-linear/solar--pip-linear.svg?raw'
import play from '@/assets/solar-linear/solar--play-linear.svg?raw'
import playbackSpeed from '@/assets/solar-linear/solar--playback-speed-linear.svg?raw'
import playlist from '@/assets/solar-linear/solar--playlist-linear.svg?raw'
import quitFullScreen from '@/assets/solar-linear/solar--quit-full-screen-linear.svg?raw'
import quitPip from '@/assets/solar-linear/solar--quit-pip-linear.svg?raw'
import refresh from '@/assets/solar-linear/solar--refresh-linear.svg?raw'
import refreshCircle from '@/assets/solar-linear/solar--refresh-circle-linear.svg?raw'
import repeat from '@/assets/solar-linear/solar--repeat-linear.svg?raw'
import restart from '@/assets/solar-linear/solar--restart-linear.svg?raw'
import settings from '@/assets/solar-linear/solar--settings-linear.svg?raw'
import skipNext from '@/assets/solar-linear/solar--skip-next-linear.svg?raw'
import skipPrevious from '@/assets/solar-linear/solar--skip-previous-linear.svg?raw'
import squareArrowRightUp from '@/assets/solar-linear/solar--square-arrow-right-up-linear.svg?raw'
import star from '@/assets/solar-linear/solar--star-linear.svg?raw'
import subtitles from '@/assets/solar-linear/solar--subtitles-linear.svg?raw'
import sun from '@/assets/solar-linear/solar--sun-linear.svg?raw'
import trashBinMinimalistic from '@/assets/solar-linear/solar--trash-bin-minimalistic-linear.svg?raw'
import videocamera from '@/assets/solar-linear/solar--videocamera-linear.svg?raw'
import volume from '@/assets/solar-linear/solar--volume-linear.svg?raw'
import volumeCross from '@/assets/solar-linear/solar--volume-cross-linear.svg?raw'
import volumeLoud from '@/assets/solar-linear/solar--volume-loud-linear.svg?raw'
import volumeSmall from '@/assets/solar-linear/solar--volume-small-linear.svg?raw'

export const solarIcons = {
  add,
  'alt-arrow-down': altArrowDown,
  'alt-arrow-left': altArrowLeft,
  'alt-arrow-right': altArrowRight,
  'alt-arrow-up': altArrowUp,
  'arrow-down': arrowDown,
  'arrow-left': arrowLeft,
  'arrow-right': arrowRight,
  'arrow-up': arrowUp,
  box,
  'chat-round-line': chatRoundLine,
  'check-circle': checkCircle,
  clapperboard,
  'cloud-storage': cloudStorage,
  'cloud-upload': cloudUpload,
  'clock-circle': clockCircle,
  close,
  'close-circle': closeCircle,
  copy,
  'danger-triangle': dangerTriangle,
  download,
  'download-minimalistic': downloadMinimalistic,
  'file-pen': filePen,
  flag,
  'flag-2': flag2,
  'folder-open': folderOpen,
  'full-screen': fullScreen,
  'headphones-round': headphonesRound,
  'high-quality': highQuality,
  'info-circle': infoCircle,
  lock,
  magnifier,
  'maximize-square': maximizeSquare,
  'menu-dots': menuDots,
  'minimize-square': minimizeSquare,
  minus,
  monitor,
  moon,
  'music-notes': musicNotes,
  muted,
  pause,
  pip,
  play,
  'playback-speed': playbackSpeed,
  playlist,
  'quit-full-screen': quitFullScreen,
  'quit-pip': quitPip,
  refresh,
  'refresh-circle': refreshCircle,
  repeat,
  restart,
  settings,
  'skip-next': skipNext,
  'skip-previous': skipPrevious,
  'square-arrow-right-up': squareArrowRightUp,
  star,
  'sort-vertical': sortVertical,
  subtitles,
  sun,
  'trash-bin-minimalistic': trashBinMinimalistic,
  videocamera,
  volume,
  'volume-cross': volumeCross,
  'volume-loud': volumeLoud,
  'volume-small': volumeSmall,
} as const

/** 可用图标名（= solarIcons 的 key，可自动补全） */
export type SolarIconName = keyof typeof solarIcons
