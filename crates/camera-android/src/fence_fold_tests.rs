//! Тесты слияния fence-ов. Закрытие дескриптора наблюдается настоящим сокетом: пока наш конец
//! `OwnedFd` открыт, чтение из парного конца даёт `WouldBlock`, после закрытия — конец потока (0 байт).

use std::cell::RefCell;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

use super::*;

/// Fence для теста: владеет дескриптором (закрывается в `Drop`) и знает свой номер.
struct TestFence {
    id: usize,
    _fd: OwnedFd,
}

/// Фабрика fence-ов: хранит парные концы, чтобы потом проверить, закрыт ли каждый дескриптор.
#[derive(Default)]
struct FenceWorld {
    peers: RefCell<Vec<UnixStream>>,
}

impl FenceWorld {
    fn new_fence(&self) -> TestFence {
        let (ours, peer) = UnixStream::pair().expect("пара сокетов");
        peer.set_nonblocking(true).expect("неблокирующий режим");
        let mut peers = self.peers.borrow_mut();
        peers.push(peer);
        TestFence {
            id: peers.len() - 1,
            _fd: ours.into(),
        }
    }

    fn fences(&self, count: usize) -> Vec<TestFence> {
        (0..count).map(|_| self.new_fence()).collect()
    }

    /// Дескриптор с этим номером ещё открыт?
    fn is_open(&self, id: usize) -> bool {
        let mut byte = [0_u8; 1];
        match (&self.peers.borrow()[id]).read(&mut byte) {
            Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
            // Конец потока (0 байт) — наш конец закрыт; данных по построению не шлют.
            Ok(read) => read != 0,
        }
    }

    fn open_ids(&self) -> Vec<usize> {
        (0..self.peers.borrow().len()).filter(|id| self.is_open(*id)).collect()
    }
}

/// Поддельный `sync_merge`: создаёт новый fence, ведёт журнал пар, может отказать на заданном шаге.
struct FakeMerger<'world> {
    world: &'world FenceWorld,
    calls: RefCell<Vec<(usize, usize)>>,
    fail_on_call: Option<usize>,
}

impl<'world> FakeMerger<'world> {
    fn new(world: &'world FenceWorld, fail_on_call: Option<usize>) -> Self {
        Self {
            world,
            calls: RefCell::new(Vec::new()),
            fail_on_call,
        }
    }
}

impl FenceMerger for FakeMerger<'_> {
    type Fence = TestFence;
    type Error = i32;

    fn merge(&self, first: &TestFence, second: &TestFence) -> Result<TestFence, i32> {
        let mut calls = self.calls.borrow_mut();
        calls.push((first.id, second.id));
        if self.fail_on_call == Some(calls.len()) {
            return Err(-22);
        }
        Ok(self.world.new_fence())
    }
}

#[test]
fn no_fences_means_nothing_to_wait_and_no_merge_calls() {
    let world = FenceWorld::default();
    let merger = FakeMerger::new(&world, None);
    assert!(matches!(fold_fences(&merger, Vec::new()), FoldOutcome::NothingToWait));
    assert!(merger.calls.borrow().is_empty());
}

#[test]
fn a_single_fence_is_passed_through_without_merging() {
    let world = FenceWorld::default();
    let merger = FakeMerger::new(&world, None);
    let FoldOutcome::Merged(result) = fold_fences(&merger, world.fences(1)) else {
        panic!("один fence должен пройти насквозь");
    };
    assert_eq!(result.id, 0, "тот же fence, не копия");
    assert!(merger.calls.borrow().is_empty());
    assert_eq!(
        world.open_ids(),
        vec![0],
        "единственный fence остаётся открытым для NDK"
    );
}

#[test]
fn two_fences_merge_into_a_new_one_and_both_inputs_are_closed() {
    let world = FenceWorld::default();
    let merger = FakeMerger::new(&world, None);
    let FoldOutcome::Merged(result) = fold_fences(&merger, world.fences(2)) else {
        panic!("слияние двух должно удаться");
    };
    assert_eq!(*merger.calls.borrow(), vec![(0, 1)]);
    assert_eq!(result.id, 2);
    assert_eq!(world.open_ids(), vec![2], "открыт только итоговый fence");
}

#[test]
fn three_fences_merge_left_to_right_and_the_intermediate_is_closed() {
    let world = FenceWorld::default();
    let merger = FakeMerger::new(&world, None);
    let FoldOutcome::Merged(result) = fold_fences(&merger, world.fences(3)) else {
        panic!("слияние трёх должно удаться");
    };
    // Сначала (0,1) → fence 3, затем (3,2) → fence 4.
    assert_eq!(*merger.calls.borrow(), vec![(0, 1), (3, 2)]);
    assert_eq!(result.id, 4);
    assert_eq!(
        world.open_ids(),
        vec![4],
        "входные и промежуточный закрыты, открыт итоговый"
    );
}

#[test]
fn failure_on_the_second_merge_closes_every_descriptor_and_returns_no_fence() {
    let world = FenceWorld::default();
    let merger = FakeMerger::new(&world, Some(2));
    let outcome = fold_fences(&merger, world.fences(4));
    assert!(matches!(
        outcome,
        FoldOutcome::Failed {
            failed_merge: 2,
            error: -22
        }
    ));
    assert_eq!(
        *merger.calls.borrow(),
        vec![(0, 1), (4, 2)],
        "после отказа слияний больше нет"
    );
    assert_eq!(
        world.open_ids(),
        Vec::<usize>::new(),
        "ни один дескриптор не остался открытым"
    );
}

#[test]
fn failure_on_the_first_merge_still_closes_the_untouched_tail() {
    let world = FenceWorld::default();
    let merger = FakeMerger::new(&world, Some(1));
    let outcome = fold_fences(&merger, world.fences(3));
    assert!(matches!(outcome, FoldOutcome::Failed { failed_merge: 1, .. }));
    assert_eq!(world.open_ids(), Vec::<usize>::new());
}
