// 云盘管理（W14 §8.6）：只读列目录。后端 drive_list 与 W13 凭据/账号体系复用。
import { invoke } from '@tauri-apps/api/core'

export interface CloudDriveEntry {
  id: string
  name: string
  is_dir: boolean
  size?: number | null
  modified_at?: number | null
}

/** 列出云盘目录（parent_id 为空串表示根目录） */
export function driveList(accountId: string, parentId = ''): Promise<CloudDriveEntry[]> {
  return invoke<CloudDriveEntry[]>('drive_list', { accountId, parentId })
}