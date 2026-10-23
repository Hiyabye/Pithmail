import { invoke, isTauri } from '@tauri-apps/api/core';

export interface MessageSummary {
  id: string;
  threadId: string;
  subject: string;
  sender: string;
  receivedAt: number;
  snippet: string;
  labels: string[];
  attachmentCount: number;
}

export interface AttachmentInfo {
  name: string;
  size: number;
}

export interface MessageDetail {
  message: MessageSummary;
  body: string;
  attachments: AttachmentInfo[];
}

export interface Overview {
  messageCount: number;
  important: MessageSummary[];
  recent: MessageSummary[];
}

export interface SyncStatus {
  phase: string;
  paused: boolean;
  discovered: number;
  downloaded: number;
  pendingBodies: number;
  pendingAttachments: number;
  error: string | null;
  complete: boolean;
  nextRetryAt: number | null;
}

export interface ConnectionStatus {
  mode: 'fixture' | 'gmail';
  email: string | null;
  oauthConfigured: boolean;
  busy: boolean;
  error: string | null;
  sync: SyncStatus | null;
}

export type Theme = 'system' | 'light' | 'dark';
export interface Settings {
  theme: Theme;
}

function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    return Promise.reject(
      new Error(
        'Open Pithmail in its desktop window. The browser preview cannot access the local mailbox.',
      ),
    );
  }
  return invoke<T>(name, args);
}

export const api = {
  listMessages: (query = '') =>
    command<MessageSummary[]>('list_messages', { query }),
  listMessagesPage: (query = '', offset = 0) =>
    command<MessageSummary[]>('list_messages_page', { query, offset }),
  threadPage: (id: string, offset = 0) =>
    command<MessageDetail[]>('get_thread_page', { id, offset }),
  connection: () => command<ConnectionStatus>('get_connection'),
  openGmail: () => command<ConnectionStatus>('open_gmail'),
  connectGmail: () => command<ConnectionStatus>('connect_gmail'),
  cancelLogin: () => command<void>('cancel_login'),
  startSync: () => command<ConnectionStatus>('start_sync'),
  pauseSync: () => command<ConnectionStatus>('pause_sync'),
  message: (id: string) => command<MessageDetail>('get_message', { id }),
  thread: (id: string) => command<MessageDetail[]>('get_thread', { id }),
  overview: () => command<Overview>('get_overview'),
  settings: () => command<Settings>('get_settings'),
  saveSettings: (settings: Settings) =>
    command<Settings>('save_settings', { settings }),
};
