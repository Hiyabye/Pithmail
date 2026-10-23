import {
  api,
  type MessageDetail,
  type MessageSummary,
  type Overview,
  type Settings,
  type Theme,
} from './api';

export type View = 'Today' | 'Inbox' | 'Search' | 'Settings';

function errorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : typeof error === 'string'
      ? error
      : 'The local mailbox could not be loaded. Please retry.';
}

export class MailboxViewModel {
  view = $state<View>('Today');
  overview = $state<Overview | null>(null);
  messages = $state<MessageSummary[]>([]);
  searchResults = $state<MessageSummary[]>([]);
  query = $state('');
  searchedQuery = $state('');
  hasSearched = $state(false);
  selectedId = $state<string | null>(null);
  detail = $state<MessageDetail | null>(null);
  thread = $state<MessageDetail[]>([]);
  settings = $state<Settings>({ theme: 'system' });
  loading = $state(false);
  searching = $state(false);
  reading = $state(false);
  saving = $state(false);
  ready = $state(false);
  error = $state<string | null>(null);
  private readRequest = 0;
  private searchRequest = 0;
  private retryOperation: (() => Promise<void>) | null = null;

  async initialize(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    this.error = null;
    try {
      const [overview, messages, settings] = await Promise.all([
        api.overview(),
        api.listMessages(),
        api.settings(),
      ]);
      this.overview = overview;
      this.messages = messages;
      this.settings = settings;
      this.applyTheme(settings.theme);
      this.ready = true;
    } catch (error) {
      this.error = errorMessage(error);
      this.retryOperation = () => this.initialize();
    } finally {
      this.loading = false;
    }
  }

  navigate(view: View): void {
    if (view !== this.view) this.closeMessage();
    this.view = view;
  }

  async search(): Promise<void> {
    const request = ++this.searchRequest;
    const query = this.query.trim();
    this.searching = true;
    this.error = null;
    try {
      const messages = await api.listMessages(query);
      if (request !== this.searchRequest) return;
      this.searchResults = messages;
      this.searchedQuery = query;
      this.hasSearched = true;
    } catch (error) {
      if (request === this.searchRequest) {
        this.error = errorMessage(error);
        this.retryOperation = () => this.search();
      }
    } finally {
      if (request === this.searchRequest) this.searching = false;
    }
  }

  async openMessage(id: string): Promise<void> {
    const request = ++this.readRequest;
    this.selectedId = id;
    this.reading = true;
    this.detail = null;
    this.thread = [];
    this.error = null;
    try {
      const detail = await api.message(id);
      if (request !== this.readRequest) return;
      this.detail = detail;
      const thread = await api.thread(detail.message.threadId);
      if (request === this.readRequest) this.thread = thread;
    } catch (error) {
      if (request === this.readRequest) {
        this.error = errorMessage(error);
        this.retryOperation = () => this.openMessage(id);
      }
    } finally {
      if (request === this.readRequest) this.reading = false;
    }
  }

  closeMessage(): void {
    ++this.readRequest;
    this.selectedId = null;
    this.detail = null;
    this.thread = [];
    this.reading = false;
  }

  async setTheme(theme: Theme): Promise<void> {
    if (this.saving) return;
    this.saving = true;
    this.error = null;
    try {
      this.settings = await api.saveSettings({ theme });
      this.applyTheme(this.settings.theme);
    } catch (error) {
      this.error = errorMessage(error);
      this.retryOperation = () => this.setTheme(theme);
    } finally {
      this.saving = false;
    }
  }

  async retry(): Promise<void> {
    await this.retryOperation?.();
  }

  private applyTheme(theme: Theme): void {
    document.documentElement.dataset.theme = theme;
  }
}
