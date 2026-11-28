//! Små hjälpare för att fördela arbete på flera trådar (stora presentationer): bilderna är oberoende av varandra.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Antal arbetstrådar: alla kärnor utom en (gränssnittet behöver en), minst en och högst sex.
pub fn workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1)).clamp(1, 6)
}

/// `f` på varje post, med upp till `workers()` trådar; resultaten i samma ordning som posterna. Med en post
/// eller en arbetstråd körs det på den anropande tråden. En panik i `f` fortsätter i den anropande tråden.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let w = workers().min(items.len());
    if w <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..w {
            // Samma stackstorlek som bildtrådarna: ritmotorn är djupt rekursiv.
            let _ = std::thread::Builder::new().stack_size(8 << 20).spawn_scoped(s, || {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    if let Ok(mut o) = out.lock() {
                        o[i] = Some(r);
                    }
                }
            });
        }
    });
    let done = out.into_inner().unwrap_or_default();
    // Saknas ett resultat (en tråd kunde inte startas eller paniken) görs resten här.
    done.into_iter().enumerate().map(|(i, r)| r.unwrap_or_else(|| f(&items[i]))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordning_och_alla_poster() {
        let v: Vec<usize> = (0..257).collect();
        let r = map(&v, |&x| x * 2);
        assert_eq!(r, (0..257).map(|x| x * 2).collect::<Vec<_>>());
        assert!(map(&Vec::<u8>::new(), |&x| x).is_empty());
        assert_eq!(map(&[5], |&x| x + 1), vec![6]);
    }
}
