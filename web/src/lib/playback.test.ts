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
