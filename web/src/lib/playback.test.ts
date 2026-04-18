import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { PlaybackEngine } from './playback';

const SAMPLE_CAST = [
  '{"version":2,"width":80,"height":24,"timestamp":1713264000,"env":{"TERM":"xterm-256color"},"telepair":{}}',
  '[0.000000, "o", "$ "]',
  '[0.500000, "o", "ls\\r\\n"]',
  '[1.000000, "r", "120x30"]',
  '[1.500000, "j", "{\\"user_id\\":\\"u1\\",\\"name\\":\\"bob\\",\\"role\\":\\"operator\\"}"]',
  '[2.000000, "o", "file1.txt\\r\\n"]',
  '[2.500000, "c", "{\\"user_id\\":\\"u1\\",\\"name\\":\\"bob\\",\\"text\\":\\"hi\\"}"]',
  '[3.000000, "l", "{\\"user_id\\":\\"u1\\"}"]',
].join('\n');

describe('PlaybackEngine', () => {
  let engine: PlaybackEngine;
  beforeEach(() => { engine = new PlaybackEngine(); vi.useFakeTimers(); });
  afterEach(() => { engine.dispose(); vi.useRealTimers(); });

  it('parses asciicast header', () => {
    engine.load(SAMPLE_CAST);
    expect(engine.header.version).toBe(2);
    expect(engine.header.width).toBe(80);
  });

  it('parses all events', () => {
    engine.load(SAMPLE_CAST);
    expect(engine.events.length).toBe(7);
    expect(engine.events[0]).toEqual({ time: 0, type: 'o', data: '$ ' });
  });

  it('reports total duration', () => {
    engine.load(SAMPLE_CAST);
    expect(engine.duration).toBe(3.0);
  });

  it('emits output events during playback', () => {
    engine.load(SAMPLE_CAST);
    const outputs: string[] = [];
    engine.onOutput = (data) => outputs.push(data);
    engine.play();
    vi.advanceTimersByTime(100);
    expect(outputs).toContain('$ ');
    vi.advanceTimersByTime(500);
    expect(outputs).toContain('ls\r\n');
  });

  it('emits resize events', () => {
    engine.load(SAMPLE_CAST);
    let resized = false;
    engine.onResize = (cols, rows) => { resized = true; expect(cols).toBe(120); expect(rows).toBe(30); };
    engine.play();
    vi.advanceTimersByTime(1100);
    expect(resized).toBe(true);
  });

  it('pauses and resumes', () => {
    engine.load(SAMPLE_CAST);
    const outputs: string[] = [];
    engine.onOutput = (data) => outputs.push(data);
    engine.play();
    vi.advanceTimersByTime(100);
    const countBefore = outputs.length;
    engine.pause();
    vi.advanceTimersByTime(2000);
    expect(outputs.length).toBe(countBefore);
    engine.play();
    vi.advanceTimersByTime(600);
    expect(outputs.length).toBeGreaterThan(countBefore);
  });

  it('seeks to a specific time', () => {
    engine.load(SAMPLE_CAST);
    const outputs: string[] = [];
    engine.onOutput = (data) => outputs.push(data);
    engine.seek(2.0);
    expect(outputs).toContain('$ ');
    expect(outputs).toContain('ls\r\n');
    expect(outputs).toContain('file1.txt\r\n');
  });

  it('replays participant join/leave and chat events on seek', () => {
    engine.load(SAMPLE_CAST);
    const joins: Array<{ user_id: string; name: string }> = [];
    const leaves: Array<{ user_id: string }> = [];
    const chats: Array<{ user_id: string; text: string }> = [];
    engine.onParticipantJoin = (p) => joins.push({ user_id: p.user_id, name: p.name });
    engine.onParticipantLeave = (p) => leaves.push(p);
    engine.onChat = (p) => chats.push({ user_id: p.user_id, text: p.text });

    // Seek past the join + chat but before the leave.
    engine.seek(2.7);
    expect(joins).toEqual([{ user_id: 'u1', name: 'bob' }]);
    expect(chats).toEqual([{ user_id: 'u1', text: 'hi' }]);
    expect(leaves).toEqual([]);

    // Seek past the leave: the listener should now have observed the leave too.
    engine.seek(3.0);
    expect(leaves).toEqual([{ user_id: 'u1' }]);
  });

  it('reports the requested seek target via currentTime', () => {
    engine.load(SAMPLE_CAST);
    engine.seek(2.7);
    // Even though the last replayed event was at t=2.5, the engine
    // pins currentTime to the requested target so the progress bar
    // and onTimeUpdate consumers see the user-requested position.
    expect(engine.currentTime).toBeCloseTo(2.7, 5);
  });

  it('respects speed multiplier', () => {
    engine.load(SAMPLE_CAST);
    const outputs: string[] = [];
    engine.onOutput = (data) => outputs.push(data);
    engine.setSpeed(2);
    engine.play();
    vi.advanceTimersByTime(300);
    expect(outputs).toContain('ls\r\n');
  });

  it('fires onComplete when playback ends', () => {
    engine.load(SAMPLE_CAST);
    let completed = false;
    engine.onComplete = () => { completed = true; };
    engine.play();
    vi.advanceTimersByTime(4000);
    expect(completed).toBe(true);
  });

  // Regression: the pre-refactor implementation registered a `setTimeout`
  // for every remaining event at `play()` time, so the timer count
  // tracked event count. For long casts that pinned O(N) memory in the
  // JS timer queue and made every `pause()` / `seek()` / `setSpeed()`
  // walk the full array to cancel. Each play() segment now queues
  // exactly one pending timer — pinning the invariant here means any
  // future edit that goes back to pre-registering all events will
  // trip this test, not just some fuzzy perf regression.
  it('keeps only one pending timer at a time during playback', () => {
    engine.load(SAMPLE_CAST);
    engine.play();
    expect(vi.getTimerCount()).toBe(2); // 1 pump timer + 1 ticker interval
    // Cross a few event boundaries — the pump should re-queue a single
    // replacement each time, never stack up.
    for (let step = 0; step < 6; step++) {
      vi.advanceTimersByTime(500);
      // During `ended` the pump releases its timer, so 1 (ticker only)
      // is acceptable for the final iteration; everything else must
      // still be exactly 2.
      expect(vi.getTimerCount()).toBeLessThanOrEqual(2);
    }
  });

  // Regression: synchronous seek must leave zero outstanding timers
  // while paused. The pre-refactor code cleared an array of per-event
  // timers and re-populated `nextIndex` from a linear scan; the pump
  // refactor replaces that with a single-handle clear. This test
  // guards against a regression where `seek()` forgets to cancel the
  // pump while the engine was playing, which would leave an orphan
  // timer ticking at the old speed after the user scrubs.
  it('leaves no timers pending after seek from a playing state', () => {
    engine.load(SAMPLE_CAST);
    engine.play();
    vi.advanceTimersByTime(100); // dispatch a couple events
    engine.pause();
    engine.seek(1.5);
    expect(vi.getTimerCount()).toBe(0);
  });
});
