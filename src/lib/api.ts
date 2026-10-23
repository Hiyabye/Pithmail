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
  message: (id: string) => command<MessageDetail>('get_message', { id }),
  thread: (id: string) => command<MessageDetail[]>('get_thread', { id }),
  overview: () => command<Overview>('get_overview'),
  settings: () => command<Settings>('get_settings'),
  saveSettings: (settings: Settings) =>
    command<Settings>('save_settings', { settings }),
};
