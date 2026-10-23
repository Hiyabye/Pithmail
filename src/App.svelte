<script lang="ts">
  import { onMount } from 'svelte';
  import MessageList from './lib/MessageList.svelte';
  import MessageReader from './lib/MessageReader.svelte';
  import SyncStatus from './lib/SyncStatus.svelte';
  import { MailboxViewModel, type View } from './lib/mailbox.svelte';
  import type { Theme } from './lib/api';

  const mailbox = new MailboxViewModel();
  const views: View[] = ['Today', 'Inbox', 'Search', 'Settings'];
  const themes: { value: Theme; label: string }[] = [
    { value: 'system', label: 'Use device theme' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
  ];
  const openMessage = (id: string) => {
    void mailbox.openMessage(id);
  };

  onMount(() => {
    void mailbox.initialize();
    return () => mailbox.dispose();
  });
</script>

<svelte:head
  ><title>Pithmail — Local mailbox</title><meta
    name="description"
    content="A private, local-first email reader."
  /></svelte:head
>

<div class="app-shell">
  <aside class="sidebar">
    <a class="brand" href="#main"
      >Pithmail<span>Less mail. More clarity.</span></a
    >
    <nav class="main-nav" aria-label="Main navigation">
      {#each views as view (view)}
        <button
          class:active={mailbox.view === view}
          aria-current={mailbox.view === view ? 'page' : undefined}
          onclick={() => mailbox.navigate(view)}>{view}</button
        >
      {/each}
    </nav>
    <div class="sidebar-footer">
      <span class="status-dot"></span><span
        >{mailbox.connection.mode === 'gmail'
          ? 'Encrypted local mailbox'
          : 'Local fixture mailbox'}<br /><span class="muted"
          >{mailbox.connection.email ?? 'No Gmail mailbox opened'}</span
        ></span
      >
    </div>
  </aside>

  <main id="main" tabindex="-1">
    <header class="page-header">
      <div>
        <p class="eyebrow">YOUR PRIVATE MAILBOX</p>
        <h1>{mailbox.view}</h1>
      </div>
      <p class="backend-status" id="startup-status" role="status">
        {mailbox.loading
          ? 'Opening local mailbox…'
          : mailbox.ready
            ? 'Local mailbox ready · offline capable'
            : 'Local mailbox unavailable'}
      </p>
    </header>

    {#if mailbox.error}
      <div class="error-banner" role="alert">
        <p>{mailbox.error}</p>
        <button
          onclick={() => {
            void mailbox.retry();
          }}
          disabled={mailbox.loading ||
            mailbox.reading ||
            mailbox.searching ||
            mailbox.connectionBusy}>Retry</button
        >
      </div>
    {/if}

    {#if mailbox.connection.mode === 'gmail' && mailbox.connection.sync && mailbox.view !== 'Settings'}
      <div class="sync-strip" role="status">
        <span
          >{mailbox.connection.sync.paused
            ? 'Download paused'
            : mailbox.connection.sync.complete
              ? 'Mailbox synchronized'
              : `Download: ${mailbox.connection.sync.phase}`} · {mailbox
            .connection.sync.downloaded} messages stored{mailbox.connection.sync
            .error
            ? ' · Attention needed'
            : ''}</span
        >
        <button class="text-button" onclick={() => mailbox.navigate('Settings')}
          >Sync settings</button
        >
      </div>
    {/if}

    {#if mailbox.view === 'Settings'}
      <div class="settings-page">
        <section class="settings-section">
          <h2>Appearance</h2>
          <p class="muted">Choose a comfortable reading environment.</p>
          <label for="theme">Theme</label>
          <select
            id="theme"
            value={mailbox.settings.theme}
            disabled={!mailbox.ready || mailbox.saving}
            onchange={(event) => {
              void mailbox.setTheme(event.currentTarget.value as Theme);
            }}
          >
            {#each themes as theme (theme.value)}<option value={theme.value}
                >{theme.label}</option
              >{/each}
          </select>
          {#if mailbox.saving}<p role="status">Saving theme…</p>{/if}
        </section>
        <section class="settings-section">
          <h2>Gmail connection</h2>
          <p class="muted">
            One personal Gmail account, read-only. Connecting opens your system
            browser. Your tokens stay in the OS credential store.
          </p>
          {#if mailbox.connection.email}<p class="account-address">
              {mailbox.connection.email}
            </p>{/if}
          {#if mailbox.connection.error}<p class="sync-error" role="alert">
              {mailbox.connection.error}
            </p>{/if}
          <div class="button-row">
            {#if mailbox.connection.mode === 'fixture'}
              <button
                id="open-gmail"
                disabled={!mailbox.ready ||
                  mailbox.connectionBusy ||
                  mailbox.connection.busy}
                onclick={() => {
                  void mailbox.openGmail();
                }}>Open encrypted Gmail mailbox</button
              >
            {/if}
            <button
              id="connect-gmail"
              disabled={!mailbox.ready ||
                mailbox.connectionBusy ||
                mailbox.connection.busy ||
                !mailbox.connection.oauthConfigured}
              onclick={() => {
                void mailbox.connectGmail();
              }}>Connect / reconnect Gmail</button
            >
            {#if mailbox.loggingIn}
              <button
                onclick={() => {
                  void mailbox.cancelLogin();
                }}>Cancel sign-in</button
              >
            {/if}
          </div>
          {#if mailbox.connectionBusy}<p role="status">
              {mailbox.loggingIn
                ? 'Waiting for sign-in in your browser…'
                : 'Opening the local mailbox…'}
            </p>{/if}
          <p class="muted">
            Opening stored mail works offline and does not contact Google.
            Startup uses fixtures until you explicitly unlock Gmail; restarting
            returns to fixtures. Use Sync now to request network
            synchronization.
          </p>
          <details class="oauth-help">
            <summary
              >Local OAuth setup{mailbox.connection.oauthConfigured
                ? ' · configured'
                : ' · needed to connect'}</summary
            >
            <p class="muted">
              Enable Gmail API in your Google Cloud project and create a Desktop
              app OAuth client. Keep its downloaded JSON outside this repository
              in an owner-only file (permissions 0600). Set <code
                >PITHMAIL_OAUTH_CONFIG</code
              > to its path before launching Pithmail. Add your account as a test
              user if needed. Never paste credentials into source files.
            </p>
            <p class="muted">
              Google applications in Testing mode can require reconnection after
              seven days. This is separate from your offline encrypted mail.
            </p>
          </details>
          {#if mailbox.connection.sync}
            <SyncStatus
              status={mailbox.connection.sync}
              busy={mailbox.connectionBusy || mailbox.connection.busy}
              onstart={() => {
                void mailbox.startSync();
              }}
              onpause={() => {
                void mailbox.pauseSync();
              }}
            />
          {:else if mailbox.connection.mode === 'gmail'}
            <button
              disabled={mailbox.connectionBusy}
              onclick={() => {
                void mailbox.startSync();
              }}>Sync now</button
            >
          {/if}
        </section>
        <section class="settings-section">
          <h2>Mailbox & privacy</h2>
          <dl class="settings-facts">
            <div>
              <dt>Source</dt>
              <dd>
                {mailbox.connection.mode === 'gmail'
                  ? 'Gmail · read-only local copy'
                  : 'Synthetic fixture emails only'}
              </dd>
            </div>
            <div>
              <dt>Storage</dt>
              <dd>
                {mailbox.connection.mode === 'gmail'
                  ? 'Local SQLCipher database, separate from fixtures'
                  : 'Local, disposable SQLite database'}
              </dd>
            </div>
            <div>
              <dt>Encryption</dt>
              <dd>
                {mailbox.connection.mode === 'gmail'
                  ? 'Mail, attachments, and search indexes are encrypted. The key stays in the OS credential store; no plaintext fallback.'
                  : 'Not enabled for synthetic data. Gmail uses a separate encrypted database.'}
              </dd>
            </div>
            <div>
              <dt>Network</dt>
              <dd>
                Only explicit Gmail sign-in/synchronization; no remote AI,
                telemetry, or analytics
              </dd>
            </div>
          </dl>
          <p class="muted">
            Messages render as text; remote images and active HTML are never
            loaded. Confirmed remote deletions remove the local message and
            attachments on synchronization. Gmail Spam and Trash remain
            available locally but are excluded from Today. Losing the encryption
            key requires downloading the mailbox again.
          </p>
        </section>
        <section class="settings-section">
          <h2>Coming in later milestones</h2>
          <p class="muted">
            Local llama.cpp analysis, installed models, per-task model
            assignments, and energy profiles are not implemented yet. Reading
            and search do not require AI.
          </p>
        </section>
      </div>
    {:else}
      <div
        class="mail-workspace"
        class:message-open={mailbox.selectedId !== null}
      >
        <section
          class="mail-column"
          aria-label={`${mailbox.view} messages`}
          aria-busy={mailbox.loading || mailbox.searching}
        >
          {#if mailbox.view === 'Today'}
            <div class="overview-intro">
              <h2>Your mailbox, at a glance</h2>
              <p>
                {mailbox.overview
                  ? `${mailbox.overview.messageCount} messages stored locally.`
                  : 'Opening your local mailbox…'} No AI required.
              </p>
              <p class="muted">
                Important and recent mail, excluding Spam and Trash. This is a
                deterministic overview, not an AI briefing or extracted task
                list.
              </p>
            </div>
            <h3 class="list-heading">Important</h3>
            <MessageList
              messages={mailbox.overview?.important ?? []}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.loading ? 'Loading…' : 'No important messages.'}
            />
            <h3 class="list-heading">Recent mail</h3>
            <MessageList
              messages={mailbox.overview?.recent ?? []}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.loading ? 'Loading…' : 'No recent messages.'}
            />
          {:else if mailbox.view === 'Inbox'}
            <div class="list-intro">
              <h2>All local mail</h2>
              <p class="muted">
                Includes archived, sent, Spam, and Trash messages.
              </p>
            </div>
            <MessageList
              messages={mailbox.messages}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.loading ? 'Loading…' : 'No local messages.'}
            />
            {#if mailbox.inboxHasMore}<div class="load-more">
                <button
                  disabled={mailbox.loadingMore}
                  onclick={() => {
                    void mailbox.loadMore(false);
                  }}
                  >{mailbox.loadingMore
                    ? 'Loading…'
                    : 'Load more messages'}</button
                >
              </div>{/if}
          {:else}
            <form
              class="search-form"
              onsubmit={(event) => {
                event.preventDefault();
                void mailbox.search();
              }}
            >
              <label for="mail-search">Search local mail</label>
              <div class="search-controls">
                <input
                  id="mail-search"
                  type="search"
                  bind:value={mailbox.query}
                  maxlength="512"
                  placeholder="Subject, sender, or message text"
                /><button
                  type="submit"
                  disabled={!mailbox.ready || mailbox.searching}
                  >{mailbox.searching ? 'Searching…' : 'Search'}</button
                >
              </div>
              <p class="muted">
                Keyword search, entirely offline. English and Korean supported.
                Leave blank to show all mail.
              </p>
            </form>
            <p class="results-label" role="status">
              {mailbox.searchResults.length} results{mailbox.searchedQuery
                ? ` for “${mailbox.searchedQuery}”`
                : ''}
            </p>
            <MessageList
              messages={mailbox.searchResults}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.hasSearched
                ? 'No matching local messages. Try another keyword.'
                : 'Search your downloaded messages above.'}
            />
            {#if mailbox.searchHasMore}<div class="load-more">
                <button
                  disabled={mailbox.loadingMore || mailbox.searching}
                  onclick={() => {
                    void mailbox.loadMore(true);
                  }}
                  >{mailbox.loadingMore
                    ? 'Loading…'
                    : 'Load more results'}</button
                >
              </div>{/if}
          {/if}
        </section>
        <MessageReader
          detail={mailbox.detail}
          thread={mailbox.thread}
          loading={mailbox.reading}
          hasMore={mailbox.threadHasMore}
          loadingMore={mailbox.loadingThread}
          onmore={() => {
            void mailbox.loadMoreThread();
          }}
          onselect={openMessage}
          onclose={() => mailbox.closeMessage()}
        />
      </div>
    {/if}
  </main>
</div>
