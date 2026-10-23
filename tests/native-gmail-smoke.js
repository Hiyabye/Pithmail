// Debug-only synthetic encrypted mailbox. No native secrets or Google requests.
void (async () => {
  const wait = async (predicate, description) => {
    for (let i = 0; i < 100; i++) {
      if (predicate()) return;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error(description);
  };
  const assert = (value, description) => {
    if (!value) throw new Error(description);
  };
  const button = (text) =>
    [...document.querySelectorAll('button')].find(
      (item) => item.textContent.trim() === text,
    );
  try {
    await wait(
      () =>
        document
          .querySelector('#startup-status')
          ?.textContent.includes('ready'),
      'encrypted startup',
    );
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const status = await invoke('get_connection');
    assert(
      status.mode === 'gmail' && status.email === 'synthetic@example.test',
      'real storage IPC',
    );
    assert(
      status.sync.pendingAttachments === 105 &&
        !status.sync.complete &&
        status.sync.paused,
      'pending parts not complete',
    );
    button('Inbox').click();
    await wait(
      () => document.querySelectorAll('.message-item').length === 100,
      'first 100 messages',
    );
    button('Load more messages').click();
    await wait(
      () => document.querySelectorAll('.message-item').length === 105,
      'pagination without 200 cap',
    );
    assert(
      document
        .querySelector('.message-labels')
        .textContent.includes('Synthetic label'),
      'provider label name',
    );
    document.querySelector('.message-item').click();
    await wait(
      () =>
        document
          .querySelector('.mail-body')
          ?.textContent.includes('Original offline text'),
      'encrypted original reader',
    );
    await wait(
      () => document.querySelectorAll('.thread-item').length === 100,
      'thread first page',
    );
    button('Load more conversation messages').click();
    await wait(
      () => document.querySelectorAll('.thread-item').length === 105,
      'thread second page',
    );
    assert(
      document
        .querySelector('.attachments')
        .textContent.includes('pending.bin'),
      'pending attachment metadata',
    );
    button('Search').click();
    await wait(
      () =>
        document.querySelector('#mail-search')?.getBoundingClientRect().width >
        0,
      'visible search',
    );
    const input = document.querySelector('#mail-search');
    input.value = '자료구조';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector('.search-form').requestSubmit();
    await wait(
      () => document.querySelectorAll('.message-item').length === 100,
      'encrypted Korean FTS first page',
    );
    button('Load more results').click();
    await wait(
      () => document.querySelectorAll('.message-item').length === 105,
      'encrypted FTS second page',
    );
    button('Settings').click();
    await wait(
      () =>
        document
          .querySelector('.settings-page')
          ?.textContent.includes('SQLCipher'),
      'encrypted privacy status',
    );
    window.__pithmailSmoke = 'pass';
  } catch (error) {
    window.__pithmailSmoke = `FAIL: ${error.message}`;
  }
})();
