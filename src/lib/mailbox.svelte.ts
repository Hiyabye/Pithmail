import {
  api,
  type ConnectionStatus,
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

function unique(messages: MessageSummary[]): MessageSummary[] {
  return [
    ...new Map(messages.map((message) => [message.id, message])).values(),
  ];
}

function uniqueThread(messages: MessageDetail[]): MessageDetail[] {
  return [...new Map(messages.map((item) => [item.message.id, item])).values()];
}

export class MailboxViewModel {
  view = $state<View>('Today');
  overview = $state<Overview | null>(null);
  messages = $state<MessageSummary[]>([]);
  searchResults = $state<MessageSummary[]>([]);
  query = $state('');
  searchedQuery = $state('');
  hasSearched = $state(false);
  inboxHasMore = $state(false);
  searchHasMore = $state(false);
  threadHasMore = $state(false);
  loadingMore = $state(false);
  loadingThread = $state(false);
  selectedId = $state<string | null>(null);
  detail = $state<MessageDetail | null>(null);
  thread = $state<MessageDetail[]>([]);
  settings = $state<Settings>({ theme: 'system' });
  connection = $state<ConnectionStatus>({
    mode: 'fixture',
    email: null,
    oauthConfigured: false,
    busy: false,
    error: null,
    sync: null,
  });
  connectionBusy = $state(false);
  loggingIn = $state(false);
  loading = $state(false);
  searching = $state(false);
  reading = $state(false);
  saving = $state(false);
  ready = $state(false);
  error = $state<string | null>(null);
  private readRequest = 0;
  private searchRequest = 0;
  private inboxRequest = 0;
  private generation = 0;
  private retryOperation: (() => Promise<void>) | null = null;
  private pollTimer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;

  get syncing(): boolean {
    const sync = this.connection.sync;
    return (
      this.connection.mode === 'gmail' &&
      sync !== null &&
      !sync.paused &&
      !sync.complete &&
      !sync.error
    );
  }

  async initialize(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    this.error = null;
    try {
      const [overview, messages, settings, connection] = await Promise.all([
        api.overview(),
        api.listMessagesPage(),
        api.settings(),
        api.connection(),
      ]);
      if (this.disposed) return;
      this.overview = overview;
      this.messages = messages;
      this.inboxHasMore = messages.length === 100;
      this.settings = settings;
      this.connection = connection;
      this.applyTheme(settings.theme);
      this.ready = true;
      this.schedulePoll();
    } catch (error) {
      this.fail(error, () => this.initialize());
    } finally {
      this.loading = false;
    }
  }

  dispose(): void {
    this.disposed = true;
    this.stopPolling();
    ++this.generation;
    ++this.readRequest;
    ++this.searchRequest;
    ++this.inboxRequest;
  }

  navigate(view: View): void {
    if (view !== this.view) this.closeMessage();
    this.view = view;
    if (this.connection.mode === 'gmail') {
      void this.refreshLocal().catch((error: unknown) =>
        this.fail(error, () => this.refreshLocal()),
      );
    }
  }

  async search(): Promise<void> {
    const request = ++this.searchRequest;
    const generation = this.generation;
    const query = this.query.trim();
    this.searching = true;
    this.error = null;
    try {
      const messages = await api.listMessagesPage(query);
      if (request !== this.searchRequest || generation !== this.generation)
        return;
      this.searchResults = messages;
      this.searchHasMore = messages.length === 100;
      this.searchedQuery = query;
      this.hasSearched = true;
    } catch (error) {
      if (request === this.searchRequest) this.fail(error, () => this.search());
    } finally {
      if (request === this.searchRequest) this.searching = false;
    }
  }

  async loadMore(search: boolean): Promise<void> {
    if (this.loadingMore || this.searching) return;
    this.loadingMore = true;
    this.error = null;
    const generation = this.generation;
    const request = search ? ++this.searchRequest : ++this.inboxRequest;
    const query = search ? this.searchedQuery : '';
    const previous = search ? this.searchResults : this.messages;
    try {
      const next = await api.listMessagesPage(query, previous.length);
      if (
        generation !== this.generation ||
        request !== (search ? this.searchRequest : this.inboxRequest)
      )
        return;
      if (search) {
        this.searchResults = unique([...previous, ...next]);
        this.searchHasMore = next.length === 100;
      } else {
        this.messages = unique([...previous, ...next]);
        this.inboxHasMore = next.length === 100;
      }
    } catch (error) {
      this.fail(error, () => this.loadMore(search));
    } finally {
      this.loadingMore = false;
    }
  }

  async openMessage(id: string): Promise<void> {
    const request = ++this.readRequest;
    this.selectedId = id;
    this.reading = true;
    this.detail = null;
    this.thread = [];
    this.threadHasMore = false;
    this.loadingThread = false;
    this.error = null;
    try {
      const detail = await api.message(id);
      if (request !== this.readRequest) return;
      this.detail = detail;
      const thread = await api.threadPage(detail.message.threadId);
      if (request === this.readRequest) {
        this.thread = thread;
        this.threadHasMore = thread.length === 100;
      }
    } catch (error) {
      if (request === this.readRequest)
        this.fail(error, () => this.openMessage(id));
    } finally {
      if (request === this.readRequest) this.reading = false;
    }
  }

  async loadMoreThread(): Promise<void> {
    if (!this.detail || this.loadingThread) return;
    const request = this.readRequest;
    const threadId = this.detail.message.threadId;
    this.loadingThread = true;
    try {
      const next = await api.threadPage(threadId, this.thread.length);
      if (request !== this.readRequest) return;
      const all = [...this.thread, ...next];
      this.thread = uniqueThread(all);
      this.threadHasMore = next.length === 100;
    } catch (error) {
      if (request === this.readRequest)
        this.fail(error, () => this.loadMoreThread());
    } finally {
      if (request === this.readRequest) this.loadingThread = false;
    }
  }

  closeMessage(): void {
    ++this.readRequest;
    this.selectedId = null;
    this.detail = null;
    this.thread = [];
    this.threadHasMore = false;
    this.reading = false;
    this.loadingThread = false;
  }

  async setTheme(theme: Theme): Promise<void> {
    if (this.saving) return;
    this.saving = true;
    this.error = null;
    try {
      this.settings = await api.saveSettings({ theme });
      this.applyTheme(this.settings.theme);
    } catch (error) {
      this.fail(error, () => this.setTheme(theme));
    } finally {
      this.saving = false;
    }
  }

  async openGmail(): Promise<void> {
    await this.connectionAction(
      () => api.openGmail(),
      () => this.openGmail(),
      true,
    );
  }

  async connectGmail(): Promise<void> {
    this.loggingIn = true;
    try {
      await this.connectionAction(
        () => api.connectGmail(),
        () => this.connectGmail(),
        true,
      );
    } finally {
      this.loggingIn = false;
    }
  }

  async cancelLogin(): Promise<void> {
    try {
      await api.cancelLogin();
    } catch (error) {
      this.fail(error, () => this.cancelLogin());
    }
  }

  async startSync(): Promise<void> {
    await this.connectionAction(
      () => api.startSync(),
      () => this.startSync(),
    );
  }

  async pauseSync(): Promise<void> {
    await this.connectionAction(
      () => api.pauseSync(),
      () => this.pauseSync(),
    );
  }

  async retry(): Promise<void> {
    await this.retryOperation?.();
  }

  private fail(error: unknown, retry: () => Promise<void>): void {
    if (this.disposed) return;
    this.error = errorMessage(error);
    this.retryOperation = retry;
  }

  private async connectionAction(
    action: () => Promise<ConnectionStatus>,
    retry: () => Promise<void>,
    reload = false,
  ): Promise<void> {
    if (this.connectionBusy) return;
    this.connectionBusy = true;
    this.error = null;
    this.stopPolling();
    try {
      const previousMode = this.connection.mode;
      this.connection = await action();
      if (this.connection.mode !== previousMode) {
        ++this.generation;
        ++this.inboxRequest;
        ++this.searchRequest;
        this.closeMessage();
        this.messages = [];
        this.searchResults = [];
        this.hasSearched = false;
        this.searchHasMore = false;
        this.searching = false;
        this.loadingMore = false;
      }
      if (reload) {
        this.loading = true;
        const [overview, messages, settings] = await Promise.all([
          api.overview(),
          api.listMessagesPage(),
          api.settings(),
        ]);
        this.overview = overview;
        this.messages = messages;
        this.inboxHasMore = messages.length === 100;
        this.settings = settings;
        this.applyTheme(settings.theme);
      }
    } catch (error) {
      this.fail(error, retry);
    } finally {
      this.loading = false;
      this.connectionBusy = false;
      this.schedulePoll();
    }
  }

  private stopPolling(): void {
    if (this.pollTimer !== null) clearTimeout(this.pollTimer);
    this.pollTimer = null;
  }

  private schedulePoll(): void {
    this.stopPolling();
    if (!this.disposed && this.syncing) {
      this.pollTimer = setTimeout(() => {
        void this.poll();
      }, 2000);
    }
  }

  private async poll(): Promise<void> {
    try {
      this.connection = await api.connection();
      await this.refreshLocal();
    } catch (error) {
      this.fail(error, () => this.poll());
    } finally {
      this.schedulePoll();
    }
  }

  private async refreshLocal(): Promise<void> {
    const generation = this.generation;
    const overview = await api.overview();
    if (this.disposed || generation !== this.generation) return;
    this.overview = overview;
    const isSearch = this.view === 'Search' && this.hasSearched;
    if (
      (this.view === 'Inbox' || isSearch) &&
      !this.loadingMore &&
      !this.searching
    ) {
      const request = isSearch ? ++this.searchRequest : ++this.inboxRequest;
      const query = isSearch ? this.searchedQuery : '';
      const loaded = isSearch
        ? this.searchResults.length
        : this.messages.length;
      const pages = Math.max(1, Math.ceil(loaded / 100));
      const messages: MessageSummary[] = [];
      let hasMore = false;
      for (let page = 0; page < pages; page++) {
        const next = await api.listMessagesPage(query, page * 100);
        messages.push(...next);
        hasMore = next.length === 100;
        if (!hasMore) break;
      }
      if (
        generation !== this.generation ||
        request !== (isSearch ? this.searchRequest : this.inboxRequest)
      )
        return;
      if (isSearch) {
        this.searchResults = unique(messages);
        this.searchHasMore = hasMore;
      } else {
        this.messages = unique(messages);
        this.inboxHasMore = hasMore;
      }
    }
    const id = this.selectedId;
    const request = this.readRequest;
    if (id && !this.reading) {
      const detail = await api.message(id);
      if (request === this.readRequest && generation === this.generation)
        this.detail = detail;
    }
  }

  private applyTheme(theme: Theme): void {
    document.documentElement.dataset.theme = theme;
  }
}
