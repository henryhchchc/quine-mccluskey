//! Boolean function minimizer based on [Quine-McCluskey algorithm](https://en.wikipedia.org/wiki/Quine%E2%80%93McCluskey_algorithm).
//!
//! # Usage
//!
//! ````
//! use quine_mccluskey as qmc;
//!
//! let mut solutions = qmc::minimize(
//!     &qmc::DEFAULT_VARIABLES[..3],
//!     &[0, 5],        // minterms
//!     &[1, 3, 4, 6],  // maxterms
//!     qmc::SOP,
//!     false,
//!     None
//! )
//! .unwrap();
//!
//! assert_eq!(
//!     solutions.pop().unwrap().to_string(),
//!     "(A ∧ C) ∨ (~A ∧ ~C)"
//! );
//! ````
//!
//! [`minimize`] is sufficient for all use cases. But also check [`minimize_minterms`] and
//! [`minimize_maxterms`] to see if they are more suitable for your use case.
//!
//! # Feature flags
//!
//! * `serde` -- Derives the [`Serialize`] and [`Deserialize`] traits for structs and enums.
//!

mod group;
mod implicant;
mod petrick;
mod prime_implicant_chart;
mod solution;

pub use solution::Solution;
pub use solution::Variable;
#[doc(hidden)]
pub use Form::{POS, SOP};

use std::collections::HashSet;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use group::Group;
use implicant::{Implicant, VariableSort};
use petrick::Petrick;
use prime_implicant_chart::PrimeImplicantChart;

/// Minimizes the boolean function represented by the given `minterms` and `maxterms`.
///
/// Returns a list of equally minimal boolean expressions.
///
/// `minterms` represent the terms whose output is 1 and `maxterms` represent the terms whose output is 0.
/// The rest of the terms are inferred to be don't care conditions.
///
/// `form` determines whether the minimized expression is of the form [`SOP`] (Sum of Products) or [`POS`] (Product of Sums).
///
/// If `find_all_solutions` is `true`, prime implicant chart simplification based on row/column dominance step will be skipped
/// and all solutions will be returned. This on average makes the algorithm less efficient and more likely to get stuck.
///
/// If `timeout` is specified, the function will return [`Error::Timeout`] if the solution is not found within the given time.
///
/// # Example
///
/// Let's minimize the boolean function expressed by the following truth table:
///
/// | A | B | C | Output |
/// |:-:|:-:|:-:|:------:|
/// | 0 | 0 | 0 | 1      |
/// | 0 | 0 | 1 | 0      |
/// | 0 | 1 | 0 | X      |
/// | 0 | 1 | 1 | 0      |
/// | 1 | 0 | 0 | 0      |
/// | 1 | 0 | 1 | 1      |
/// | 1 | 1 | 0 | 0      |
/// | 1 | 1 | 1 | X      |
///
/// In [`SOP`] form:
///
/// ```
/// use quine_mccluskey as qmc;
///
/// let mut solutions = qmc::minimize(
///     &qmc::DEFAULT_VARIABLES[..3],
///     &[0, 5],
///     &[1, 3, 4, 6],
///     qmc::SOP,
///     false,
///     None
/// )
/// .unwrap();
///
/// assert_eq!(
///     solutions.pop().unwrap().to_string(),
///     "(A ∧ C) ∨ (~A ∧ ~C)"
/// );
///
/// ```
///
/// And in [`POS`] form:
///
/// ```
/// # use quine_mccluskey as qmc;
/// let mut solutions = qmc::minimize(
///     &qmc::DEFAULT_VARIABLES[..3],
///     &[0, 5],
///     &[1, 3, 4, 6],
///     qmc::POS,
///     false,
///     None
/// )
/// .unwrap();
///
/// assert_eq!(
///     solutions.pop().unwrap().to_string(),
///     "(A ∨ ~C) ∧ (~A ∨ C)"
/// );
/// ````
pub fn minimize<T: AsRef<str>>(
    variables: &[T],
    minterms: &[u32],
    maxterms: &[u32],
    form: Form,
    find_all_solutions: bool,
    timeout: Option<Duration>,
) -> Result<Vec<Solution>, Error> {
    let variables = own_variables(variables);
    let variable_count = variables.len() as u32;

    let minterms = HashSet::from_iter(minterms.iter().copied());
    let maxterms = HashSet::from_iter(maxterms.iter().copied());

    validate_input(&variables, &minterms, &maxterms)?;

    let dont_cares = get_dont_cares(variable_count, &minterms, &maxterms);
    let terms = if form == SOP { minterms } else { maxterms };

    let internal_solutions = minimize_internal_with_timeout(
        variable_count,
        terms,
        dont_cares,
        form,
        find_all_solutions,
        timeout,
    )?;

    Ok(internal_solutions
        .iter()
        .map(|solution| Solution::new(solution, &variables, form))
        .collect())
}

/// Minimizes the boolean function represented by the given `minterms` and `dont_cares`.
///
/// The only other difference to [`minimize`] is that it doesn't take an argument for form,
/// instead always returns in [`SOP`] form.
///
/// # Example
///
/// Let's minimize the boolean function expressed by the following truth table:
///
/// | A | B | C | Output |
/// |:-:|:-:|:-:|:------:|
/// | 0 | 0 | 0 | 1      |
/// | 0 | 0 | 1 | 0      |
/// | 0 | 1 | 0 | X      |
/// | 0 | 1 | 1 | 0      |
/// | 1 | 0 | 0 | 0      |
/// | 1 | 0 | 1 | 1      |
/// | 1 | 1 | 0 | 0      |
/// | 1 | 1 | 1 | X      |
///
/// ```
/// use quine_mccluskey as qmc;
///
/// let mut solutions = qmc::minimize_minterms(
///     &qmc::DEFAULT_VARIABLES[..3],
///     &[0, 5],
///     &[2, 7],
///     false,
///     None
/// )
/// .unwrap();
///
/// assert_eq!(
///     solutions.pop().unwrap().to_string(),
///     "(A ∧ C) ∨ (~A ∧ ~C)"
/// );
/// ````
pub fn minimize_minterms<T: AsRef<str>>(
    variables: &[T],
    minterms: &[u32],
    dont_cares: &[u32],
    find_all_solutions: bool,
    timeout: Option<Duration>,
) -> Result<Vec<Solution>, Error> {
    let variables = own_variables(variables);
    let variable_count = variables.len() as u32;

    let minterms = HashSet::from_iter(minterms.iter().copied());
    let dont_cares = HashSet::from_iter(dont_cares.iter().copied());

    validate_input(&variables, &minterms, &dont_cares)?;

    let internal_solutions = minimize_internal_with_timeout(
        variable_count,
        minterms,
        dont_cares,
        SOP,
        find_all_solutions,
        timeout,
    )?;

    Ok(internal_solutions
        .iter()
        .map(|solution| Solution::new(solution, &variables, SOP))
        .collect())
}

/// Minimizes the boolean function represented by the given `maxterms` and `dont_cares`.
///
/// The only other difference to [`minimize`] is that it doesn't take an argument for form,
/// instead always returns in [`POS`] form.
///
/// # Example
///
/// Let's minimize the boolean function expressed by the following truth table:
///
/// | A | B | C | Output |
/// |:-:|:-:|:-:|:------:|
/// | 0 | 0 | 0 | 1      |
/// | 0 | 0 | 1 | 0      |
/// | 0 | 1 | 0 | X      |
/// | 0 | 1 | 1 | 0      |
/// | 1 | 0 | 0 | 0      |
/// | 1 | 0 | 1 | 1      |
/// | 1 | 1 | 0 | 0      |
/// | 1 | 1 | 1 | X      |
///
/// ```
/// use quine_mccluskey as qmc;
///
/// let mut solutions = qmc::minimize_maxterms(
///     &qmc::DEFAULT_VARIABLES[..3],
///     &[1, 3, 4, 6],
///     &[2, 7],
///     false,
///     None
/// )
/// .unwrap();
///
/// assert_eq!(
///     solutions.pop().unwrap().to_string(),
///     "(A ∨ ~C) ∧ (~A ∨ C)"
/// );
/// ````
pub fn minimize_maxterms<T: AsRef<str>>(
    variables: &[T],
    maxterms: &[u32],
    dont_cares: &[u32],
    find_all_solutions: bool,
    timeout: Option<Duration>,
) -> Result<Vec<Solution>, Error> {
    let variables = own_variables(variables);
    let variable_count = variables.len() as u32;

    let maxterms = HashSet::from_iter(maxterms.iter().copied());
    let dont_cares = HashSet::from_iter(dont_cares.iter().copied());

    validate_input(&variables, &maxterms, &dont_cares)?;

    let internal_solutions = minimize_internal_with_timeout(
        variable_count,
        maxterms,
        dont_cares,
        POS,
        find_all_solutions,
        timeout,
    )?;

    Ok(internal_solutions
        .iter()
        .map(|solution| Solution::new(solution, &variables, POS))
        .collect())
}

/// The form of a boolean expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Form {
    /// Sum of Products
    SOP,
    /// Product of Sums
    POS,
}

/// All letters of the English alphabet in uppercase.
pub static DEFAULT_VARIABLES: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];

/// Error types for bad input and timeout.
#[derive(Debug, thiserror::Error, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Error {
    /// The number of variables was less than 1 or greater than `DEFAULT_VARIABLES.len()`.
    #[error("Invalid variable count: {0} (expected 1 <= variables.len() <= {})", DEFAULT_VARIABLES.len())]
    InvalidVariableCount(usize),
    /// Variable was 0, 1, empty string or string with leading or trailing whitespaces.
    #[error("0, 1, empty string and strings with leading or trailing whitespaces are not allowed as variables.")]
    InvalidVariable,
    /// There were duplicate variables.
    #[error("Duplicate variables are not allowed: {0:?}")]
    DuplicateVariables(HashSet<String>),
    /// There were terms out of bounds for the given number of variables.
    #[error("Terms out of bounds: {:?} (expected < {} for {} variables)", offending_terms, 1 << variable_count, variable_count)]
    TermOutOfBounds {
        offending_terms: HashSet<u32>,
        variable_count: usize,
    },
    /// There were conflicting terms between the given term sets.
    #[error("Conflicting terms between term sets: {0:?}")]
    TermConflict(HashSet<u32>),
    /// Could not find the solution in time.
    #[error("Could not find the solution in time.")]
    Timeout,
}

fn minimize_internal_with_timeout(
    variable_count: u32,
    terms: HashSet<u32>,
    dont_cares: HashSet<u32>,
    form: Form,
    find_all_solutions: bool,
    timeout: Option<Duration>,
) -> Result<Vec<Vec<Implicant>>, Error> {
    let Some(timeout) = timeout else {
        return Ok(minimize_internal(
            variable_count,
            &terms,
            &dont_cares,
            form,
            find_all_solutions,
        ));
    };

    let (sender, receiver) = mpsc::channel();
    let timeout_sender = sender.clone();

    thread::spawn(move || {
        sender
            .send(Ok(minimize_internal(
                variable_count,
                &terms,
                &dont_cares,
                form,
                find_all_solutions,
            )))
            .unwrap()
    });

    thread::spawn(move || {
        thread::sleep(timeout);
        timeout_sender.send(Err(Error::Timeout)).unwrap();
    });

    receiver.recv().unwrap()
}

fn minimize_internal(
    variable_count: u32,
    terms: &HashSet<u32>,
    dont_cares: &HashSet<u32>,
    form: Form,
    find_all_solutions: bool,
) -> Vec<Vec<Implicant>> {
    let prime_implicants = find_prime_implicants(variable_count, terms, dont_cares, form);
    let mut prime_implicant_chart = PrimeImplicantChart::new(prime_implicants, dont_cares);
    let essential_prime_implicants = prime_implicant_chart.simplify(find_all_solutions);
    let petrick_solutions = Petrick::solve(&prime_implicant_chart);

    let mut solutions = petrick_solutions
        .iter()
        .map(|solution| [essential_prime_implicants.as_slice(), solution].concat())
        .collect::<Vec<_>>();

    for solution in &mut solutions {
        solution.variable_sort(form);
        assert!(check_solution(terms, dont_cares, solution));
    }

    solutions
}

fn find_prime_implicants(
    variable_count: u32,
    terms: &HashSet<u32>,
    dont_cares: &HashSet<u32>,
    form: Form,
) -> Vec<Implicant> {
    let terms = terms.union(dont_cares).copied().collect();
    let mut groups = Group::group_terms(variable_count, &terms, form);
    let mut prime_implicants = vec![];

    loop {
        let next_groups = (0..groups.len() - 1)
            .map(|i| groups[i].combine(&groups[i + 1]))
            .collect();

        prime_implicants.extend(
            groups
                .iter()
                .flat_map(|group| group.get_prime_implicants(dont_cares)),
        );

        if groups.iter().all(|group| !group.was_combined()) {
            break;
        }

        groups = next_groups;
    }

    prime_implicants
}

fn get_dont_cares(
    variable_count: u32,
    minterms: &HashSet<u32>,
    maxterms: &HashSet<u32>,
) -> HashSet<u32> {
    let all_terms: HashSet<u32> = HashSet::from_iter(0..1 << variable_count);
    let cares = minterms.union(maxterms).copied().collect();

    all_terms.difference(&cares).copied().collect()
}

fn check_solution(terms: &HashSet<u32>, dont_cares: &HashSet<u32>, solution: &[Implicant]) -> bool {
    let covered_terms =
        HashSet::from_iter(solution.iter().flat_map(|implicant| implicant.get_terms()));
    let terms_with_dont_cares = terms.union(dont_cares).copied().collect();

    terms.is_subset(&covered_terms) && covered_terms.is_subset(&terms_with_dont_cares)
}

fn own_variables<T: AsRef<str>>(variables: &[T]) -> Vec<String> {
    variables
        .iter()
        .map(|variable| variable.as_ref().to_owned())
        .collect()
}

fn validate_input(
    variables: &[String],
    terms1: &HashSet<u32>,
    terms2: &HashSet<u32>,
) -> Result<(), Error> {
    if variables.is_empty() || variables.len() > DEFAULT_VARIABLES.len() {
        return Err(Error::InvalidVariableCount(variables.len()));
    }

    for variable in variables {
        if variable == "0" || variable == "1" || variable.is_empty() || variable != variable.trim()
        {
            return Err(Error::InvalidVariable);
        }
    }

    let mut duplicates = HashSet::new();

    for i in 0..variables.len() {
        for j in i + 1..variables.len() {
            if variables[i] == variables[j] {
                duplicates.insert(variables[i].clone());
            }
        }
    }

    if !duplicates.is_empty() {
        return Err(Error::DuplicateVariables(duplicates));
    }

    let all_terms: HashSet<u32> = terms1.union(terms2).copied().collect();
    let terms_out_of_bounds: HashSet<u32> = all_terms
        .into_iter()
        .filter(|&term| term >= 1 << variables.len())
        .collect();

    if !terms_out_of_bounds.is_empty() {
        return Err(Error::TermOutOfBounds {
            offending_terms: terms_out_of_bounds,
            variable_count: variables.len(),
        });
    }

    let conflicts: HashSet<u32> = terms1.intersection(terms2).copied().collect();

    if !conflicts.is_empty() {
        return Err(Error::TermConflict(conflicts));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use rand::Rng;

    use super::*;

    #[test]
    fn test_minimize_exhaustive() {
        for variable_count in 1..=3 {
            let term_combinations = generate_terms_exhaustive(variable_count);

            for terms in &term_combinations {
                for form in [SOP, POS] {
                    for find_all_solutions in [true, false] {
                        minimize_and_print_solutions(
                            variable_count,
                            &terms.0,
                            &terms.1,
                            form,
                            find_all_solutions,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_minimize_random() {
        for variable_count in 4..=5 {
            let term_combinations = generate_terms_random(variable_count, 1000);

            for terms in &term_combinations {
                for form in [SOP, POS] {
                    for find_all_solutions in [true, false] {
                        minimize_and_print_solutions(
                            variable_count,
                            &terms.0,
                            &terms.1,
                            form,
                            find_all_solutions,
                        );
                    }
                }
            }
        }
    }

    // #[test]
    // fn test_minimize_specific() {
    //     let variable_count = 1;
    //     let form = SOP;
    //     let find_all_solutions = true;
    //     let minterms = [];
    //     let maxterms = [];

    //     minimize_and_print_solutions(
    //         variable_count,
    //         &minterms,
    //         &maxterms,
    //         form,
    //         find_all_solutions,
    //     );
    // }

    #[test]
    fn test_find_prime_implicants() {
        fn test(
            variable_count: u32,
            minterms: &[u32],
            maxterms: &[u32],
            form: Form,
            expected: &[&str],
        ) {
            let minterms = HashSet::from_iter(minterms.iter().copied());
            let maxterms = HashSet::from_iter(maxterms.iter().copied());

            let dont_cares = get_dont_cares(variable_count, &minterms, &maxterms);
            let terms = if form == SOP { minterms } else { maxterms };

            let result = find_prime_implicants(variable_count, &terms, &dont_cares, form);

            assert_eq!(
                result.into_iter().collect::<HashSet<_>>(),
                HashSet::from_iter(expected.iter().map(|str| Implicant::from_str(str)))
            );
        }

        test(1, &[], &[0, 1], SOP, &[]);
        test(1, &[0], &[1], SOP, &["0"]);
        test(1, &[1], &[0], SOP, &["1"]);
        test(1, &[0, 1], &[], SOP, &["-"]);
        test(1, &[], &[], SOP, &[]);
        test(1, &[], &[0], SOP, &[]);
        test(1, &[], &[1], SOP, &[]);
        test(1, &[0], &[], SOP, &["-"]);
        test(1, &[1], &[], SOP, &["-"]);

        test(1, &[0, 1], &[], POS, &[]);
        test(1, &[1], &[0], POS, &["0"]);
        test(1, &[0], &[1], POS, &["1"]);
        test(1, &[], &[0, 1], POS, &["-"]);
        test(1, &[], &[], POS, &[]);
        test(1, &[0], &[], POS, &[]);
        test(1, &[1], &[], POS, &[]);
        test(1, &[], &[0], POS, &["-"]);
        test(1, &[], &[1], POS, &["-"]);

        test(2, &[0, 3], &[2], SOP, &["0-", "-1"]);

        test(
            3,
            &[1, 2, 5],
            &[3, 4, 7],
            SOP,
            &["00-", "0-0", "-01", "-10"],
        );

        test(
            4,
            &[2, 4, 5, 7, 9],
            &[3, 6, 10, 12, 15],
            SOP,
            &["00-0", "01-1", "10-1", "0-0-", "-00-", "--01"],
        );
    }

    fn minimize_and_print_solutions(
        variable_count: u32,
        minterms: &[u32],
        maxterms: &[u32],
        form: Form,
        find_all_solutions: bool,
    ) {
        let dont_cares = Vec::from_iter(get_dont_cares(
            variable_count,
            &HashSet::from_iter(minterms.iter().copied()),
            &HashSet::from_iter(maxterms.iter().copied()),
        ));

        println!(
            "form: {:?}, find_all_solutions: {}, variable_count: {}, minterms: {:?}, maxterms: {:?}, dont_cares: {:?}",
            form, find_all_solutions, variable_count, minterms, maxterms, dont_cares
        );

        let solutions = minimize(
            &DEFAULT_VARIABLES[..variable_count as usize],
            minterms,
            maxterms,
            form,
            find_all_solutions,
            None,
        )
        .unwrap();

        println!(
            "{:#?}",
            solutions
                .iter()
                .map(|solution| solution.to_string())
                .collect::<Vec<_>>()
        );
    }

    fn generate_terms_exhaustive(variable_count: u32) -> Vec<(Vec<u32>, Vec<u32>)> {
        let mut generated_terms = vec![];
        let all_terms: HashSet<u32> = HashSet::from_iter(0..1 << variable_count);

        for i in 0..=all_terms.len() {
            let minterm_combinations = all_terms
                .iter()
                .copied()
                .combinations(i)
                .map(HashSet::from_iter);

            for minterms in minterm_combinations {
                let other_terms: HashSet<u32> = all_terms.difference(&minterms).copied().collect();

                for j in 0..=other_terms.len() {
                    let maxterm_combinations = other_terms
                        .iter()
                        .copied()
                        .combinations(j)
                        .map(HashSet::<u32>::from_iter);

                    generated_terms.extend(maxterm_combinations.map(|maxterms| {
                        (Vec::from_iter(minterms.clone()), Vec::from_iter(maxterms))
                    }));
                }
            }
        }

        generated_terms
    }

    fn generate_terms_random(variable_count: u32, count: u32) -> Vec<(Vec<u32>, Vec<u32>)> {
        let mut generated_terms = vec![];
        let mut rng = rand::thread_rng();

        for _ in 0..count {
            let mut all_terms = Vec::from_iter(0..1 << variable_count);
            let mut minterms = vec![];
            let mut maxterms = vec![];

            for _ in 0..all_terms.len() {
                let term = all_terms.swap_remove(rng.gen_range(0..all_terms.len()));
                let choice = rng.gen_range(1..=3);

                if choice == 1 {
                    minterms.push(term);
                } else if choice == 2 {
                    maxterms.push(term);
                }
            }

            generated_terms.push((minterms, maxterms));
        }

        generated_terms
    }
}
