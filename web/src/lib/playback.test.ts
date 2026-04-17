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
});
