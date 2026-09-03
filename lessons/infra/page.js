export function createLessonSurface(lesson) {
  const hud = document.querySelector('#hud');
  const errorSurface = document.querySelector('#error');
  const resultElement = document.createElement('script');
  resultElement.id = 'lesson-result';
  resultElement.type = 'application/json';
  document.head.append(resultElement);
  let failed = false;
  let result = {
    lesson,
    status: 'BOOTING',
    assertions: {},
  };

  function publish(patch) {
    result = {
      ...result,
      ...patch,
      assertions: { ...result.assertions, ...(patch.assertions ?? {}) },
    };
    globalThis.__LESSON_RESULT__ = result;
    document.documentElement.dataset.lessonStatus = result.status;
    resultElement.textContent = JSON.stringify(result);
  }

  function progress(message) {
    if (!failed) hud.textContent = `LESSON ${String(lesson).padStart(2, '0')} · ${message}`;
  }

  function pass(details, assertions) {
    if (failed) return;
    const assertionMap = assertions && typeof assertions === 'object' && !Array.isArray(assertions)
      ? assertions
      : {};
    const entries = Object.entries(assertionMap);
    const rejected = entries
      .filter(([, value]) => value !== true)
      .map(([name, value]) => `${name}=${JSON.stringify(value)}`);
    if (entries.length === 0) rejected.push('assertions=<empty>');
    if (rejected.length > 0) {
      const error = new Error(`Cannot publish PASS: ${rejected.join(', ')}`);
      fail(error, { details, assertions: assertionMap });
      throw error;
    }
    publish({ status: 'PASS', details, assertions });
    hud.textContent = [
      `LESSON ${String(lesson).padStart(2, '0')} · PASS`,
      details.adapter,
      `${details.width} × ${details.height} · ${details.format}`,
      'H · hide diagnostics',
    ].join('\n');
  }

  function fail(value, context = {}) {
    if (failed) return;
    failed = true;
    const error = value instanceof Error ? value : new Error(String(value));
    publish({ ...context, status: 'FAIL', error: error.message });
    errorSurface.textContent = `${error.message}\n\nOpen DevTools for the full stack.`;
    console.error(error);
  }

  publish({});
  addEventListener('keydown', (event) => {
    if (event.code === 'KeyH') document.body.classList.toggle('hide-hud');
  });
  addEventListener('error', (event) => fail(event.error ?? event.message));
  addEventListener('unhandledrejection', (event) => fail(event.reason));

  return { progress, pass, fail };
}
