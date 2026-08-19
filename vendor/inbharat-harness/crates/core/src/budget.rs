//! Non-bypassable execution budgets.

use crate::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use crate::routing::ExecutionLevel;
use std::time::{Duration, Instant};

/// Hard limits for one run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetLimits {
    pub max_steps: u32,
    pub max_tool_calls: u32,
    pub max_rounds: u32,
    pub max_jobs: u32,
    pub max_subagent_depth: u8,
    pub max_output_bytes: usize,
    pub max_duration: Duration,
}

impl BudgetLimits {
    /// Conservative defaults scaled by execution level.
    #[must_use]
    pub const fn for_level(level: ExecutionLevel) -> Self {
        match level {
            ExecutionLevel::L0 => Self {
                max_steps: 1,
                max_tool_calls: 0,
                max_rounds: 1,
                max_jobs: 0,
                max_subagent_depth: 0,
                max_output_bytes: 32 * 1024,
                max_duration: Duration::from_secs(15),
            },
            ExecutionLevel::L1 => Self {
                max_steps: 1,
                max_tool_calls: 1,
                max_rounds: 1,
                max_jobs: 0,
                max_subagent_depth: 0,
                max_output_bytes: 64 * 1024,
                max_duration: Duration::from_secs(20),
            },
            ExecutionLevel::L2 => Self {
                max_steps: 8,
                max_tool_calls: 12,
                max_rounds: 1,
                max_jobs: 0,
                max_subagent_depth: 0,
                max_output_bytes: 256 * 1024,
                max_duration: Duration::from_secs(60),
            },
            ExecutionLevel::L3 => Self {
                max_steps: 32,
                max_tool_calls: 64,
                max_rounds: 8,
                max_jobs: 8,
                max_subagent_depth: 2,
                max_output_bytes: 2 * 1024 * 1024,
                max_duration: Duration::from_secs(300),
            },
        }
    }
}

/// Mutable accounting object owned by one run.
#[derive(Debug)]
pub struct Budget {
    limits: BudgetLimits,
    started: Instant,
    steps: u32,
    tool_calls: u32,
    rounds: u32,
    jobs: u32,
    output_bytes: usize,
}

impl Budget {
    #[must_use]
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            started: Instant::now(),
            steps: 0,
            tool_calls: 0,
            rounds: 0,
            jobs: 0,
            output_bytes: 0,
        }
    }

    #[must_use]
    pub const fn limits(&self) -> BudgetLimits {
        self.limits
    }

    pub fn reserve_step(&mut self) -> HarnessResult<u32> {
        self.check_deadline("budget.step")?;
        reserve_count(&mut self.steps, self.limits.max_steps, "steps")
    }

    pub fn reserve_tool_call(&mut self) -> HarnessResult<u32> {
        self.check_deadline("budget.tool")?;
        reserve_count(
            &mut self.tool_calls,
            self.limits.max_tool_calls,
            "tool_calls",
        )
    }

    pub fn reserve_round(&mut self) -> HarnessResult<u32> {
        self.check_deadline("budget.round")?;
        reserve_count(&mut self.rounds, self.limits.max_rounds, "rounds")
    }

    pub fn reserve_job(&mut self) -> HarnessResult<u32> {
        self.check_deadline("budget.job")?;
        reserve_count(&mut self.jobs, self.limits.max_jobs, "jobs")
    }

    pub fn account_output(&mut self, bytes: usize) -> HarnessResult<usize> {
        self.check_deadline("budget.output")?;
        let next = self.output_bytes.saturating_add(bytes);
        if next > self.limits.max_output_bytes {
            return Err(exceeded("output_bytes", self.limits.max_output_bytes));
        }
        self.output_bytes = next;
        Ok(next)
    }

    pub fn check_deadline(&self, operation: &str) -> HarnessResult<()> {
        if self.started.elapsed() > self.limits.max_duration {
            return Err(Failure::new(
                ErrorCode::Timeout,
                FailureClass::Resource,
                operation,
                "run deadline exceeded",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub const fn steps_used(&self) -> u32 {
        self.steps
    }

    #[must_use]
    pub const fn tool_calls_used(&self) -> u32 {
        self.tool_calls
    }

    #[must_use]
    pub const fn output_bytes(&self) -> usize {
        self.output_bytes
    }
}

fn reserve_count(counter: &mut u32, max: u32, dimension: &str) -> HarnessResult<u32> {
    if *counter >= max {
        return Err(exceeded(dimension, max));
    }
    *counter += 1;
    Ok(*counter)
}

fn exceeded(dimension: &str, limit: impl ToString) -> Failure {
    Failure::new(
        ErrorCode::BudgetExceeded,
        FailureClass::Resource,
        "budget.reserve",
        format!("{dimension} budget exhausted"),
    )
    .with_detail("dimension", dimension)
    .with_detail("limit", limit.to_string())
}
