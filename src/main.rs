use std::{
    cell::UnsafeCell,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier, RwLock,
    },
    thread,
    time::Instant,
};

trait Universe {
    fn name(&self) -> &'static str;
    fn step(&mut self);
    fn step_count(&self) -> u64;
}

struct SingleThreadUniverse {
    worlds: Vec<World>,
    step_count: u64,
}

impl SingleThreadUniverse {
    fn new(num_fast: usize, num_slow: usize) -> Self {
        let mut worlds = Vec::with_capacity(num_fast + num_slow);
        worlds.resize_with(num_slow, || World::new(WorldType::Slow));
        worlds.resize_with(num_slow + num_fast, || World::new(WorldType::Fast));
        Self {
            step_count: 0,
            worlds,
        }
    }
}

impl Universe for SingleThreadUniverse {
    fn step(&mut self) {
        for world in self.worlds.iter_mut() {
            self.step_count += 1;
            world.step();
        }
    }

    fn name(&self) -> &'static str {
        "Single Thread"
    }

    fn step_count(&self) -> u64 {
        self.step_count
    }
}

struct ThreadPerWorldUniverse {
    step_count: Arc<AtomicU64>,
    frame_begin: Arc<Barrier>,
    frame_end: Arc<Barrier>,
}

impl ThreadPerWorldUniverse {
    fn new(num_fast: usize, num_slow: usize) -> Self {
        let num_worlds = num_fast + num_slow;
        let step_count = Arc::new(AtomicU64::new(0));
        let frame_begin = Arc::new(Barrier::new(num_worlds + 1));
        let frame_end = Arc::new(Barrier::new(num_worlds + 1));

        let make_world = |world_type| {
            let step_count = step_count.clone();
            let frame_begin = frame_begin.clone();
            let frame_end = frame_end.clone();
            let mut world = World::new(world_type);

            thread::spawn(move || loop {
                frame_begin.wait();
                world.step();
                step_count.fetch_add(1, Ordering::Relaxed);
                frame_end.wait();
            });
        };

        for _ in 0..num_fast {
            make_world(WorldType::Fast);
        }

        for _ in 0..num_slow {
            make_world(WorldType::Slow);
        }

        Self {
            step_count,
            frame_begin,
            frame_end,
        }
    }
}

impl Universe for ThreadPerWorldUniverse {
    fn name(&self) -> &'static str {
        "Single Thread Per World"
    }

    fn step(&mut self) {
        self.frame_begin.wait();
        self.frame_end.wait();
    }

    fn step_count(&self) -> u64 {
        self.step_count.load(Ordering::Relaxed)
    }
}

struct WorldHolder(UnsafeCell<World>);

struct TaskPerWorldUniverse {
    world_index: Arc<AtomicUsize>,
    step_count: Arc<AtomicU64>,
    frame_begin: Arc<Barrier>,
    frame_end: Arc<Barrier>,
}

unsafe impl Send for WorldHolder {}
unsafe impl Sync for WorldHolder {}

impl TaskPerWorldUniverse {
    fn new(num_fast: usize, num_slow: usize) -> Self {
        let num_worlds = num_fast + num_slow;
        let mut worlds = Vec::with_capacity(num_worlds);
        worlds.resize_with(num_slow, || {
            WorldHolder(UnsafeCell::new(World::new(WorldType::Slow)))
        });
        worlds.resize_with(num_slow + num_fast, || {
            WorldHolder(UnsafeCell::new(World::new(WorldType::Fast)))
        });
        let worlds = Arc::new(RwLock::new(worlds));

        let step_count = Arc::new(AtomicU64::new(0));
        let world_index = Arc::new(AtomicUsize::new(0));

        let worker_count = 8;
        let frame_begin = Arc::new(Barrier::new(worker_count + 1));
        let frame_end = Arc::new(Barrier::new(worker_count + 1));

        for _ in 0..worker_count {
            let worlds = worlds.clone();
            let step_count = step_count.clone();
            let world_index = world_index.clone();
            let frame_begin = frame_begin.clone();
            let frame_end = frame_end.clone();

            thread::spawn(move || loop {
                frame_begin.wait();

                let worlds = worlds.read().unwrap();

                'frame: loop {
                    let world_index = world_index.fetch_add(1, Ordering::AcqRel);
                    if world_index >= num_worlds {
                        break 'frame;
                    }

                    step_count.fetch_add(1, Ordering::Relaxed);

                    let world = &worlds[world_index];
                    let world = unsafe { &mut *world.0.get() };

                    world.step();
                }

                frame_end.wait();
            });
        }

        Self {
            world_index,
            step_count,
            frame_begin,
            frame_end,
        }
    }
}

impl Universe for TaskPerWorldUniverse {
    fn name(&self) -> &'static str {
        "Task Per World"
    }

    fn step(&mut self) {
        self.world_index.store(0, Ordering::Relaxed);
        self.frame_begin.wait();
        self.frame_end.wait();
    }

    fn step_count(&self) -> u64 {
        self.step_count.load(Ordering::Relaxed)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum WorldType {
    Fast,
    Slow,
}

struct World {
    world_type: WorldType,
    frame_number: u64,
    accumulator: u64,
    increment: u64,
}

impl World {
    fn new(world_type: WorldType) -> Self {
        Self {
            world_type,
            frame_number: 0,
            accumulator: 0x853c49e6748fea9b,
            increment: 0xda3e39cb94b95bdb,
        }
    }

    fn step(&mut self) {
        let iters = match self.world_type {
            WorldType::Fast => 1_000_000,
            WorldType::Slow => 10_000_000,
        };
        for _ in 0..iters {
            self.accumulator = self
                .accumulator
                .wrapping_mul(0x5851f42d4c957f2)
                .wrapping_add(self.increment);
        }

        self.frame_number += 1;
    }
}

fn main() {
    let num_fast = 100;
    let num_slow = 30;

    println!("Stepping {} fast, and {} slow worlds.", num_fast, num_slow);

    let mut universes: [&mut dyn Universe; 3] = [
        &mut SingleThreadUniverse::new(num_fast, num_slow),
        &mut ThreadPerWorldUniverse::new(num_fast, num_slow),
        &mut TaskPerWorldUniverse::new(num_fast, num_slow),
    ];

    let num_frames = 30;

    let mut frame_times = Vec::with_capacity(num_frames as usize);

    for verse in universes.iter_mut() {
        println!("Running {}", verse.name());

        let now = Instant::now();
        for _ in 0..num_frames {
            let start_frame = Instant::now();
            verse.step();
            frame_times.push(start_frame.elapsed());
        }

        assert_eq!(
            num_frames * (num_fast + num_slow) as u64,
            verse.step_count()
        );

        let duration = now.elapsed();

        struct Stats {
            min: f64,
            max: f64,
            sum: f64,
            count: f64,
        }

        impl Stats {
            fn new() -> Self {
                Self {
                    min: f64::MAX,
                    max: f64::MIN,
                    sum: 0.0,
                    count: 0.0,
                }
            }

            fn average(&self) -> f64 {
                self.sum / self.count
            }
        }

        let stats = frame_times.iter().fold(Stats::new(), |acc, &x| {
            let secs = x.as_secs_f64();
            Stats {
                min: acc.min.min(secs),
                max: acc.min.max(secs),
                sum: acc.sum + secs,
                count: acc.count + 1.0,
            }
        });

        frame_times.clear();

        println!(
            "took {:?} for {} steps over {} frames. {:?} per frame",
            duration,
            verse.step_count(),
            num_frames,
            duration / num_frames as u32
        );

        println!(
            "min: {:.2}ms max: {:.2}ms avg: {:.2}ms",
            stats.min * 1000.0,
            stats.max * 1000.0,
            stats.average() * 1000.0
        );

        println!();
    }
}
