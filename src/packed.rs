// Copyright (c) 2016 rust-threshold-secret-sharing developers
//
// Licensed under the Apache License, Version 2.0
// <LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0> or the MIT
// license <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. All files in the project carrying such notice may not be copied,
// modified, or distributed except according to those terms.

//! Packed variant of secret sharing, allowing to share efficiently several values together.

use numtheory::{mod_pow, fft2_inverse, fft3};
use rand;

/// Packed variant of the secret sharing.
///
/// In Shamir scheme, one single value (one number) is set as the 0-th
/// coefficient of a polynomial, and the evaluation of this polynomial
/// at different point is shared to different sharees. Once enough shares
/// (degree+1) are put together, the polynomial can be uniquely determined
/// and evaluated in 0 to reconstruct the secret.
///
/// The idea behing the packed scheme is to "fix" more values of the
/// polynomial to represent more secret values. We could for instance pick
/// evaluation at 0, -1, -2 to encode three values, find a polynomial of
/// high-enough degree going through these points, then evaluate it on 1, 2...
/// to generate enough shares.
///
/// But operations on polynomial are expensive (quadratic) in the general case.
/// By careful picking of evaluation points and using Fast Fourier Transform,
/// most of our operation can be kept under `O(n.log(n))`.
///
/// * secrets are positioned on 2^n roots of unity
/// * shares are read on 3^n roots of unity
///
/// Except from the evaluation in `1`, a point that we do not use, these two
/// sets are distinct, so no share exposes coincidentaly any secret.
///
/// So there exist constraints between the various parameters:
///
/// * `prime` must be big enough to handle the shared values
/// * `secret_count + threshold + 1` (aka reconstruct_limit) must be a power of 2
/// * `share_count + 1` must be a power of 3
/// * `omega_secrets` must be a `reconstruct_limit()`-th root of unity
/// * `omega_shares` must be a `(share_count+1)`-th root of unity
#[derive(Debug,Copy,Clone,PartialEq)]
pub struct PackedSecretSharing {
    // abstract properties
    /// security threshold
    pub threshold: usize,
    /// number of shares to generate
    pub share_count: usize,
    /// number of secrets in each share
    pub secret_count: usize,

    // implementation configuration
    /// prime field to use
    pub prime: i64,
    /// `reconstruct_limit`-th principal root of unity in Z_p
    pub omega_secrets: i64,
    /// `secret_count+1`-th principal root of unity in Z_p
    pub omega_shares: i64,
}

/// Example of tiny PSS settings, for sharing 3 secrets 8 ways, with
/// a security threshold of 4.
pub static PSS_4_8_3: PackedSecretSharing = PackedSecretSharing {
    threshold: 4,
    share_count: 8,
    secret_count: 3,
    prime: 433,
    omega_secrets: 354,
    omega_shares: 150,
};

/// Example of small PSS settings, for sharing 3 secrets 26 ways, with
/// a security threshold of 4.
pub static PSS_4_26_3: PackedSecretSharing = PackedSecretSharing {
    threshold: 4,
    share_count: 26,
    secret_count: 3,
    prime: 433,
    omega_secrets: 354,
    omega_shares: 17,
};

/// Example of PSS settings, for sharing 100 secrets 728 ways, with
/// a security threshold of 156.
pub static PSS_155_728_100: PackedSecretSharing = PackedSecretSharing {
    threshold: 155,
    share_count: 728,
    secret_count: 100,
    prime: 746497,
    omega_secrets: 95660,
    omega_shares: 610121,
};

/// Example of PSS settings, for sharing 100 secrets 19682 ways, with
/// a security threshold of 156.
pub static PSS_155_19682_100: PackedSecretSharing = PackedSecretSharing {
    threshold: 155,
    share_count: 19682,
    secret_count: 100,
    prime: 5038849,
    omega_secrets: 4318906,
    omega_shares: 1814687,
};

impl PackedSecretSharing {
    /// minimum number of shares required to reconstruct secret
    ///
    /// (secret_count + threshold + 1)
    pub fn reconstruct_limit(&self) -> usize {
        self.secret_count + self.threshold + 1
    }

    /// Computes shares for the vector of secrets.
    ///
    /// It is assumed that `secret` is equal in len to `secret_count` (the
    /// code will assert otherwise). It is safe to pad with anything, including
    /// zeros.
    pub fn share(&self, secrets: &[i64]) -> Vec<i64> {
        assert_eq!(secrets.len(), self.secret_count);
        // sample polynomial
        let mut poly = self.sample_polynomial(secrets);
        // .. and extend it
        poly.extend(vec![0; self.share_count + 1 - self.reconstruct_limit()]);
        // evaluate polynomial to generate shares
        let mut shares = self.evaluate_polynomial(poly);
        // .. but remove first element since it should not be used as a share (it's always 1)
        shares.remove(0);
        // return
        assert_eq!(shares.len(), self.share_count);
        shares
    }

    fn sample_polynomial(&self, secrets: &[i64]) -> Vec<i64> {
        // sample randomness
        //  - for cryptographic use we should use OsRng as dictated here
        //    https://doc.rust-lang.org/rand/rand/index.html#cryptographic-security
        use rand::distributions::Sample;
        let mut range = rand::distributions::range::Range::new(0, self.prime - 1);
        let mut rng = rand::OsRng::new().unwrap();
        let randomness: Vec<i64> =
            (0..self.threshold).map(|_| range.sample(&mut rng) as i64).collect();
        // recover polynomial
        let coefficients = self.recover_polynomial(secrets, randomness);
        coefficients
    }

    fn recover_polynomial(&self, secrets: &[i64], randomness: Vec<i64>) -> Vec<i64> {
        // fix the value corresponding to point 1
        let mut values: Vec<i64> = vec![0];
        // let the subsequent values correspond to the secrets
        values.extend(secrets);
        // fill in with random values
        values.extend(randomness);
        // run backward FFT to recover polynomial in coefficient representation
        assert_eq!(values.len(), self.reconstruct_limit());
        let coefficients = fft2_inverse(&values, self.omega_secrets, self.prime);
        coefficients
    }

    fn evaluate_polynomial(&self, coefficients: Vec<i64>) -> Vec<i64> {
        assert_eq!(coefficients.len(), self.share_count + 1);
        let points = fft3(&coefficients, self.omega_shares, self.prime);
        points
    }

    /// Reconstruct the secret vector from enough shares.
    ///
    /// `indices` and `shares` must be of the same size, and strictly more than
    /// `threshold` (it will assert if otherwise).
    ///
    /// `indices` is the rank of the known shares from the `share` method
    /// output, while `values` are the actual values of these shares.
    ///
    /// The result is of length `secret_count`.
    pub fn reconstruct(&self, indices: &[usize], shares: &[i64]) -> Vec<i64> {
        assert!(shares.len() == indices.len());
        assert!(shares.len() >= self.reconstruct_limit());
        let shares_points: Vec<i64> =
            indices.iter().map(|&x| mod_pow(self.omega_shares, x as u32 + 1, self.prime)).collect();
        // interpolate using Newton's method
        use numtheory::{newton_interpolation_general, newton_evaluate};
        // TODO optimise by using Newton-equally-space variant
        let poly = newton_interpolation_general(&shares_points, &shares, self.prime);
        // evaluate at omega_secrets points to recover secrets
        // TODO optimise to avoid re-computation of power
        let secrets = (1..self.reconstruct_limit())
            .map(|e| mod_pow(self.omega_secrets, e as u32, self.prime))
            .map(|point| newton_evaluate(&poly, point, self.prime))
            .take(self.secret_count)
            .collect();
        secrets
    }
}


#[cfg(test)]
mod tests {

    use super::*;
    use numtheory::*;

    #[test]
    fn test_recover_polynomial() {
        let ref pss = PSS_4_8_3;
        let secrets = vec![1, 2, 3];
        let randomness = vec![8, 8, 8, 8];  // use fixed randomness
        let poly = pss.recover_polynomial(&secrets, randomness);
        assert_eq!(positivise(&poly, pss.prime), positivise(&[113, -382, -172, 267, -325, 432, 388, -321], pss.prime));
    }

    #[test]
    #[cfg_attr(rustfmt, rustfmt_skip)]
    fn test_evaluate_polynomial() {
        let ref pss = PSS_4_26_3;
        let poly = vec![113, 51, 261, 267, 108, 432, 388, 112, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let points = positivise(&pss.evaluate_polynomial(poly), pss.prime);
        assert_eq!(points, vec![ 0, 77, 230, 91, 286, 179, 337, 83, 212, 88,
                        406, 58, 425, 345, 350, 336, 430, 404, 51, 60, 305,
                        395, 84, 156, 160, 112, 422]);
    }

    #[test]
    #[cfg_attr(rustfmt, rustfmt_skip)]
    fn test_share() {
        let ref pss = PSS_4_26_3;

        // do sharing
        let secrets = vec![5, 6, 7];
        let mut shares = pss.share(&secrets);

        // manually recover secrets
        use numtheory::{fft3_inverse, mod_evaluate_polynomial};
        shares.insert(0, 0);
        let poly = fft3_inverse(&shares, PSS_4_26_3.omega_shares, PSS_4_26_3.prime);
        let recovered_secrets: Vec<i64> = (1..secrets.len() + 1)
            .map(|i| {
                mod_evaluate_polynomial(&poly,
                                        mod_pow(PSS_4_26_3.omega_secrets,
                                                i as u32,
                                                PSS_4_26_3.prime),
                                        PSS_4_26_3.prime)
            })
            .collect();

        use numtheory::positivise;
        assert_eq!(positivise(&recovered_secrets, pss.prime), secrets);
    }

    #[test]
    fn test_large_share() {
        let ref pss = PSS_155_19682_100;
        let secrets = vec![5 ; pss.secret_count];
        let shares = pss.share(&secrets);
        assert_eq!(shares.len(), pss.share_count);
    }

    #[test]
    fn test_share_reconstruct() {
        let ref pss = PSS_4_26_3;
        let secrets = vec![5, 6, 7];
        let shares = pss.share(&secrets);

        use numtheory::positivise;

        // reconstruction must work for all shares
        let indices: Vec<usize> = (0..shares.len()).collect();
        let recovered_secrets = pss.reconstruct(&indices, &shares);
        assert_eq!(positivise(&recovered_secrets, pss.prime), secrets);

        // .. and for only sufficient shares
        let indices: Vec<usize> = (0..pss.reconstruct_limit()).collect();
        let recovered_secrets = pss.reconstruct(&indices, &shares[0..pss.reconstruct_limit()]);
        assert_eq!(positivise(&recovered_secrets, pss.prime), secrets);
    }

    #[test]
    fn test_share_additive_homomorphism() {
        let ref pss = PSS_4_26_3;

        let secrets_1 = vec![1, 2, 3];
        let secrets_2 = vec![4, 5, 6];
        let shares_1 = pss.share(&secrets_1);
        let shares_2 = pss.share(&secrets_2);

        // add shares pointwise
        let shares_sum: Vec<i64> =
            shares_1.iter().zip(shares_2).map(|(a, b)| (a + b) % pss.prime).collect();

        // reconstruct sum, using same reconstruction limit
        let reconstruct_limit = pss.reconstruct_limit();
        let indices: Vec<usize> = (0..reconstruct_limit).collect();
        let shares = &shares_sum[0..reconstruct_limit];
        let recovered_secrets = pss.reconstruct(&indices, shares);

        use numtheory::positivise;
        assert_eq!(positivise(&recovered_secrets, pss.prime), vec![5, 7, 9]);
    }

    #[test]
    fn test_share_multiplicative_homomorphism() {
        let ref pss = PSS_4_26_3;

        let secrets_1 = vec![1, 2, 3];
        let secrets_2 = vec![4, 5, 6];
        let shares_1 = pss.share(&secrets_1);
        let shares_2 = pss.share(&secrets_2);

        // multiply shares pointwise
        let shares_product: Vec<i64> =
            shares_1.iter().zip(shares_2).map(|(a, b)| (a * b) % pss.prime).collect();

        // reconstruct product, using double reconstruction limit (minus one)
        let reconstruct_limit = pss.reconstruct_limit() * 2 - 1;
        let indices: Vec<usize> = (0..reconstruct_limit).collect();
        let shares = &shares_product[0..reconstruct_limit];
        let recovered_secrets = pss.reconstruct(&indices, shares);

        use numtheory::positivise;
        assert_eq!(positivise(&recovered_secrets, pss.prime), vec![4, 10, 18]);
    }

}


#[cfg(feature = "paramgen")]
pub mod paramgen {

    extern crate primal;

    #[cfg_attr(rustfmt, rustfmt_skip)]
    fn check_prime_form(min_p: usize, n: usize, m: usize, p: usize) -> bool {
        if p < min_p { return false; }

        let q = p - 1;
        if q % n != 0 { return false; }
        if q % m != 0 { return false; }

        let q = q / (n * m);
        if q % n == 0 { return false; }
        if q % m == 0 { return false; }

        return true;
    }

    #[test]
    fn test_check_prime_form() {
        assert_eq!(primal::Primes::all().find(|p| check_prime_form(198, 8, 9, *p)).unwrap(),
                   433);
    }

    fn factor(p: usize) -> Vec<usize> {
        let mut factors = vec![];
        let bound = (p as f64).sqrt().ceil() as usize;
        for f in 2..bound + 1 {
            if p % f == 0 {
                factors.push(f);
                factors.push(p / f);
            }
        }
        factors
    }

    #[test]
    fn test_factor() {
        assert_eq!(factor(40), [2, 20, 4, 10, 5, 8]);
        assert_eq!(factor(41), []);
    }

    fn find_field(min_p: usize, n: usize, m: usize) -> Option<(i64, i64)> {
        // find prime of right form
        let p = primal::Primes::all().find(|p| check_prime_form(min_p, n, m, *p)).unwrap();
        // find (any) generator
        let factors = factor(p - 1);
        for g in 2..p {
            // test generator against all factors of p-1
            let is_generator = factors.iter().all(|f| {
                use numtheory::mod_pow;
                let e = (p - 1) / f;
                mod_pow(g as i64, e as u32, p as i64) != 1  // TODO check for negative value
            });
            // return
            if is_generator {
                return Some((p as i64, g as i64));
            }
        }
        // didn't find any
        None
    }

    #[test]
    fn test_find_field() {
        assert_eq!(find_field(198, 2usize.pow(3), 3usize.pow(2)).unwrap(),
                   (433, 5));
        assert_eq!(find_field(198, 2usize.pow(3), 3usize.pow(3)).unwrap(),
                   (433, 5));
        assert_eq!(find_field(198, 2usize.pow(8), 3usize.pow(6)).unwrap(),
                   (746497, 5));
        assert_eq!(find_field(198, 2usize.pow(8), 3usize.pow(9)).unwrap(),
                   (5038849, 29));

        // assert_eq!(find_field(198, 2usize.pow(11), 3usize.pow(8)).unwrap(), (120932353, 5));
        // assert_eq!(find_field(198, 2usize.pow(13), 3usize.pow(9)).unwrap(), (483729409, 23));
    }

    fn find_roots(n: usize, m: usize, p: i64, g: i64) -> (i64, i64) {
        use numtheory::mod_pow;
        let omega_secrets = mod_pow(g, ((p - 1) / n as i64) as u32, p);
        let omega_shares = mod_pow(g, ((p - 1) / m as i64) as u32, p);
        (omega_secrets, omega_shares)
    }

    #[test]
    fn test_find_roots() {
        assert_eq!(find_roots(2usize.pow(3), 3usize.pow(2), 433, 5), (354, 150));
        assert_eq!(find_roots(2usize.pow(3), 3usize.pow(3), 433, 5), (354, 17));
    }

    pub fn generate_parameters(min_size: usize, n: usize, m: usize) -> (i64, i64, i64) {
        let (prime, g) = find_field(min_size, n, m).unwrap(); // TODO settle option business once and for all (don't remember it as needed)
        let (omega_secrets, omega_shares) = find_roots(n, m, prime, g);
        (prime, omega_secrets, omega_shares)
    }

    #[test]
    fn test_generate_parameters() {
        assert_eq!(generate_parameters(200, 2usize.pow(3), 3usize.pow(2)),
                   (433, 354, 150));
        assert_eq!(generate_parameters(200, 2usize.pow(3), 3usize.pow(3)),
                   (433, 354, 17));
    }

    use super::PackedSecretSharing;

    impl PackedSecretSharing {
        pub fn new_with_min_size(threshold: usize,
                                 secret_count: usize,
                                 share_count: usize,
                                 min_size: usize)
                                 -> PackedSecretSharing {
            let n = threshold + secret_count + 1;
            let m = share_count + 1;

            let two_power = (n as f64).log(2f64).floor() as u32;
            assert!(2usize.pow(two_power) == n);

            let three_power = (m as f64).log(3f64).floor() as u32;
            assert!(3usize.pow(three_power) == m);

            assert!(min_size >= share_count + secret_count + 1);

            let (prime, omega_secrets, omega_shares) = generate_parameters(min_size, n, m);

            PackedSecretSharing {
                threshold: threshold,
                share_count: share_count,
                secret_count: secret_count,
                prime: prime,
                omega_secrets: omega_secrets,
                omega_shares: omega_shares,
            }
        }

        pub fn new(threshold: usize,
                   secret_count: usize,
                   share_count: usize)
                   -> PackedSecretSharing {
            let min_size = share_count + secret_count + threshold + 1;
            Self::new_with_min_size(threshold, secret_count, share_count, min_size)
        }
    }

    #[test]
    fn test_new() {
        assert_eq!(PackedSecretSharing::new(155, 100, 728),
                   super::PSS_155_728_100);
        assert_eq!(PackedSecretSharing::new_with_min_size(4, 3, 8, 200),
                   super::PSS_4_8_3);
        assert_eq!(PackedSecretSharing::new_with_min_size(4, 3, 26, 200),
                   super::PSS_4_26_3);
    }

}
