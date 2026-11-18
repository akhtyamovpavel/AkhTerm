use super::{format_compact_tokens, TokenSnapshot};
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestHistory {
    samples: VecDeque<Option<(u64, u64)>>,
    serial: Option<u64>,
    measured: Option<Instant>,
}

fn percentage(read: u64, input: u64) -> u64 {
    ((100 * u128::from(read) + u128::from(input) / 2) / u128::from(input)).min(100) as u64
}

impl RequestHistory {
    pub fn record(&mut self, read: Option<u64>, input: u64, serial: Option<u64>) {
        let sample = read
            .filter(|&r| input > 0 && r <= input)
            .map(|r| (r, input));
        if serial.is_some() && serial == self.serial {
            if self.samples.back() == Some(&sample) {
                return;
            }
            self.samples.pop_back();
        }
        self.serial = serial;
        self.measured = Some(Instant::now());
        self.samples.push_back(sample);
        if self.samples.len() > 10 {
            self.samples.pop_front();
        }
    }

    pub fn claude_update(&mut self, read: Option<u64>, input: u64, repeated: bool) {
        if repeated {
            if read.is_none() {
                return;
            }
            self.samples.pop_back();
        }
        self.record(read, input, None);
    }

    pub fn badge(&self, ctx: Option<String>) -> Option<String> {
        let Some(Some((read, input))) = self.samples.back() else {
            return ctx;
        };
        let hit = format!("cache {}%", percentage(*read, *input));
        Some(ctx.map_or_else(|| hit.clone(), |ctx| format!("{ctx} · {hit}")))
    }

    pub fn detail(&self, snapshot: &TokenSnapshot) -> String {
        let Some(sample) = self.samples.back() else {
            return String::new();
        };
        let en = crate::i18n::current_locale() == crate::i18n::UiLocale::En;
        let (request, session, recent, age) = if en {
            ("Last request", "Session", "Recent", "s since measurement")
        } else {
            ("Последний запрос", "Сессия", "Последние", "с после замера")
        };
        let Some((read, input)) = sample else {
            return format!("\n    {request} · cache n/a");
        };
        let total = snapshot
            .cache_tokens
            .saturating_add(snapshot.effective_input());
        let overall = percentage(snapshot.cache_tokens, total.max(1));
        let trend: Vec<_> = self
            .samples
            .iter()
            .map(|s| s.map_or_else(|| "?".into(), |(r, i)| format!("{}%", percentage(r, i))))
            .collect();
        let secs = self.measured.map_or(0, |t| t.elapsed().as_secs());
        format!("\n    {request} · cache {} / {} · {secs} {age}\n    {session} · cache {overall}% · {recent} {}", format_compact_tokens(*read), format_compact_tokens(*input), trend.join("  "))
    }
}
