use crate::channel::Channel;
use crate::field::Field;
use crate::polynomial::Polynomial;
use crate::tree::Tree;
use num_bigint::ToBigInt;
use num_bigint::{BigInt, BigUint, Sign};
use std::collections::HashMap;
use std::rc::Rc;

pub struct FriOptions {
    pub offset: BigInt,
    pub omega: BigInt,
    pub domain_len: u32,
    pub expansion_factor: u32,
    pub colinearity_test_count: u32,
}

pub struct Fri {
    pub offset: BigInt,
    pub omega: BigInt,
    pub domain_len: u32,
    pub field: Rc<Field>,
    pub expansion_factor: u32,
    pub colinearity_test_count: u32,
    domain: Vec<BigInt>,
    round_count: u32,
}

impl Fri {
    pub fn new(options: &FriOptions, field: &Rc<Field>) -> Fri {
        // calculate number of rounds
        let mut codeword_len = options.domain_len;
        let mut round_count = 0;
        while codeword_len > options.expansion_factor
            && 4 * options.colinearity_test_count < codeword_len
        {
            codeword_len /= 2;
            round_count += 1;
        }
        Fri {
            offset: options.offset.clone(),
            omega: options.omega.clone(),
            domain_len: options.domain_len,
            field: Rc::clone(field),
            expansion_factor: options.expansion_factor,
            colinearity_test_count: options.colinearity_test_count,
            domain: field.coset(options.domain_len, &options.offset),
            round_count,
        }
    }

    pub fn domain(&self) -> &Vec<BigInt> {
        &self.domain
    }

    fn round_count(&self) -> u32 {
        self.round_count
    }

    pub fn prove(&self, codeword: &Vec<BigUint>, channel: &mut Channel) -> Vec<u32> {
        if self.domain_len != u32::try_from(codeword.len()).unwrap() {
            panic!("initial codeword does not match domain len");
        }
        let codewords = self.commit(codeword, channel);
        let top_indices = Self::sample_indices(
            &channel.prover_hash(),
            codewords[1].len().try_into().unwrap(),
            codewords[codewords.len() - 1].len().try_into().unwrap(),
            self.colinearity_test_count,
        );
        let mut indices: Vec<u32> = top_indices.clone();
        let codeword_trees: Vec<Tree> = codewords.iter().map(|word| Tree::build(word)).collect();
        for i in 0..(codewords.len() - 1) {
            indices = indices
                .iter()
                .map(|index| index % u32::try_from(codewords[i].len() >> 1).unwrap())
                .collect();
            self.query(
                &codewords[i],
                &codewords[i + 1],
                &indices,
                channel,
                &codeword_trees[i],
                &codeword_trees[i + 1],
            );
        }
        top_indices
    }

    fn query(
        &self,
        current_codeword: &Vec<BigUint>,
        next_codeword: &Vec<BigUint>,
        indices_c: &Vec<u32>,
        channel: &mut Channel,
        current_codeword_tree: &Tree,
        next_codeword_tree: &Tree,
    ) {
        let indices_a: Vec<u32> = indices_c.to_vec();
        let indices_b: Vec<u32> = indices_c
            .iter()
            .map(|val| val + ((current_codeword.len() >> 1) as u32))
            .collect();
        for i in 0..usize::try_from(self.colinearity_test_count).unwrap() {
            channel.push(&vec![
                current_codeword[usize::try_from(indices_a[i]).unwrap()].clone(),
                current_codeword[usize::try_from(indices_b[i]).unwrap()].clone(),
                next_codeword[usize::try_from(indices_c[i]).unwrap()].clone(),
            ]);
        }
        for i in 0..usize::try_from(self.colinearity_test_count).unwrap() {
            channel.push(&current_codeword_tree.open(indices_a[i]).0);
            channel.push(&current_codeword_tree.open(indices_b[i]).0);
            channel.push(&next_codeword_tree.open(indices_c[i]).0);
        }
    }

    fn commit(&self, codeword: &Vec<BigUint>, channel: &mut Channel) -> Vec<Vec<BigUint>> {
        let mut codewords: Vec<Vec<BigUint>> = Vec::new();
        let mut codeword = codeword.clone();
        let two_inv = self.field.inv(&BigInt::from(2));

        // invert the entire domain using repeated multiplications
        // e.g. 1/4 = (1/2) * (1/2)
        // 1/x^2 = (1/x) * (1/x)
        let inv_offset = self.field.inv(&self.offset);
        let inv_offset_domain = self
            .field
            .domain(&inv_offset, 2_u32.pow(self.round_count()));

        let inv_omega = self.field.inv(&self.omega);
        let inv_domain = self.field.domain(&inv_omega, self.domain_len);

        let mut exp: usize = 1;

        for x in 0..self.round_count() {
            let root = Tree::commit(&codeword);
            channel.push_single(&root);
            if x == self.round_count() - 1 {
                break;
            }
            codewords.push(codeword.clone());
            // now split the last codeword and fold into a set
            // of points from a polynomial of half the degree
            // of the previous codewords, similar to a FFT
            let alpha = self
                .field
                .sample(&channel.prover_hash().to_bigint().unwrap());
            let next_len = codeword.len() >> 1;
            codeword = codeword[0..next_len]
                .iter()
                .enumerate()
                .map(|(index, val)| {
                    let ival = val.to_bigint().unwrap();
                    let inv_omega = self
                        .field
                        .mul(&inv_offset_domain[exp], &inv_domain[exp * index]);
                    // ( (one + alpha / (offset * (omega^i)) ) * codeword[i]
                    let a = self.field.mul(
                        &ival,
                        &self
                            .field
                            .ladd(&Field::one(), &self.field.lmul(&alpha, &inv_omega)),
                    );
                    //  (one - alpha / (offset * (omega^i)) ) * codeword[len(codeword)//2 + i] ) for i in range(len(codeword)//2)]
                    let b = self.field.mul(
                        &self
                            .field
                            .lsub(&Field::one(), &self.field.lmul(&alpha, &inv_omega)),
                        &codeword[(codeword.len() >> 1) + index].to_bigint().unwrap(),
                    );
                    return self
                        .field
                        .mul(&two_inv, &self.field.ladd(&a, &b))
                        .to_biguint()
                        .unwrap();
                })
                .collect();

            exp *= 2;
        }
        channel.push(&codeword);
        codewords.push(codeword);
        codewords
    }

    fn sample_indices(seed: &BigUint, size: u32, reduced_size: u32, count: u32) -> Vec<u32> {
        if count > 2 * reduced_size {
            panic!("not enough entropy");
        }
        if count > reduced_size {
            panic!("cannot sample more indices than available");
        }

        let mut indices: Vec<u32> = Vec::new();
        let mut reduced_indices: HashMap<u32, bool> = HashMap::new();
        let mut counter: u32 = 0;
        while indices.len() < (count as usize) {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&seed.to_bytes_le());
            hasher.update(&BigUint::from(counter).to_bytes_le());
            let v = BigInt::from_bytes_le(Sign::Plus, hasher.finalize().as_bytes());
            let index = Field::bigint_to_u32(&(v % BigInt::from(size)));
            let reduced_index = index % reduced_size;
            counter += 1;
            if !reduced_indices.contains_key(&reduced_index) {
                indices.push(index);
                reduced_indices.insert(reduced_index, true);
            }
        }
        indices
    }

    pub fn verify(&self, channel: &mut Channel) -> Vec<(u32, BigUint)> {
        let mut out: Vec<(u32, BigUint)> = Vec::new();
        let mut offset = self.offset.clone();

        let mut roots: Vec<BigUint> = Vec::new();
        let mut alphas: Vec<BigUint> = Vec::new();

        for _ in 0..self.round_count() {
            roots.push(channel.pull().data[0].clone());
            alphas.push(
                self.field
                    .sample(&channel.verifier_hash().to_bigint().unwrap())
                    .to_biguint()
                    .unwrap(),
            );
        }

        let last_codeword = channel.pull().data.clone();
        if roots[roots.len() - 1] != Tree::commit(&last_codeword) {
            panic!("last codeword root mismatch");
        }

        let degree: usize =
            (last_codeword.len() / usize::try_from(self.expansion_factor).unwrap()) - 1;
        let mut last_offset = offset.clone();
        for _ in 0..(self.round_count() - 1) {
            last_offset = self.field.mul(&last_offset, &last_offset);
        }

        let omega_domain = self.field.domain(&self.omega, self.domain_len);
        let omega_start_index = 2_usize.pow(self.round_count() - 1);
        if self.field.inv(&omega_domain[omega_start_index])
            != self.field.exp(
                &omega_domain[omega_start_index],
                &BigInt::from(last_codeword.len() - 1),
            )
        {
            panic!("omega order incorrect");
        }

        let last_domain = last_codeword
            .iter()
            .enumerate()
            .map(|(index, _)| {
                return self
                    .field
                    .mul(&last_offset, &omega_domain[omega_start_index * index]);
            })
            .collect();

        let poly = Polynomial::interpolate_fft(
            &last_domain,
            &last_codeword
                .iter()
                .map(|v| v.to_bigint().unwrap())
                .collect(),
            &self.field,
        );
        for i in 0..last_domain.len() {
            if poly.eval(&last_domain[i]) != last_codeword[i].clone().to_bigint().unwrap() {
                panic!("interpolated polynomial is incorrect");
            }
        }
        if poly.degree() > degree {
            panic!("last codeword does not match polynomial of low enough degree");
        }

        let top_indices = Self::sample_indices(
            &channel.verifier_hash(),
            self.domain_len >> 1,
            self.domain_len >> (self.round_count() - 1),
            self.colinearity_test_count,
        );
        let mut colinearity_x_vals: Vec<Vec<BigInt>> = Vec::new();
        let mut colinearity_y_vals: Vec<Vec<BigInt>> = Vec::new();
        let mut exp = 1;
        for i in 0..usize::try_from(self.round_count() - 1).unwrap() {
            let indices_c: Vec<u32> = top_indices
                .iter()
                .map(|val| val % (self.domain_len >> (i + 1)))
                .collect();
            let indices_a: Vec<u32> = indices_c.clone();
            let indices_b: Vec<u32> = indices_a
                .iter()
                .map(|val| val + (self.domain_len >> (i + 1)))
                .collect();

            let mut aa: Vec<BigUint> = Vec::new();
            let mut bb: Vec<BigUint> = Vec::new();
            let mut cc: Vec<BigUint> = Vec::new();
            for j in 0..usize::try_from(self.colinearity_test_count).unwrap() {
                let y_points_msg = channel.pull();
                let ay = y_points_msg.data[0].clone();
                let by = y_points_msg.data[1].clone();
                let cy = y_points_msg.data[2].clone();
                aa.push(ay.clone());
                bb.push(by.clone());
                cc.push(cy.clone());
                if i == 0 {
                    out.push((indices_a[j], y_points_msg.data[0].clone()));
                    out.push((indices_b[j], y_points_msg.data[1].clone()));
                }

                let index_a_usize = usize::try_from(indices_a[j]).unwrap();
                let index_b_usize = usize::try_from(indices_b[j]).unwrap();
                let ax = self
                    .field
                    .mul(&offset, &omega_domain[exp * index_a_usize])
                    .clone();
                let bx = self
                    .field
                    .mul(&offset, &omega_domain[exp * index_b_usize])
                    .clone();

                let cx = alphas[usize::try_from(i).unwrap()].clone();

                colinearity_x_vals.push(vec![ax, bx, cx.to_bigint().unwrap()]);
                colinearity_y_vals.push(vec![
                    ay.to_bigint().unwrap(),
                    by.to_bigint().unwrap(),
                    cy.to_bigint().unwrap(),
                ]);
            }

            if !Polynomial::test_colinearity_batch(
                &colinearity_x_vals,
                &colinearity_y_vals,
                &self.field,
            ) {
                panic!("colinearity test failed");
            }

            for j in 0..usize::try_from(self.colinearity_test_count).unwrap() {
                Tree::verify(&roots[i], indices_a[j], &channel.pull().data, &aa[j]);
                Tree::verify(&roots[i], indices_b[j], &channel.pull().data, &bb[j]);
                Tree::verify(&roots[i + 1], indices_c[j], &channel.pull().data, &cc[j]);
            }

            exp *= 2;
            offset = self.field.mul(&offset, &offset);
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_make_verify_fri_proof() {
        let mut channel = Channel::new();
        let p = BigInt::from(1) + BigInt::from(407) * BigInt::from(2).pow(119);
        let g = BigInt::from(85408008396924667383611388730472331217_u128);
        let f = Rc::new(Field::new(p, g.clone()));
        let domain_size: u32 = 8192;
        let domain_g = f.generator(&BigInt::from(domain_size));

        let fri = Fri::new(
            &FriOptions {
                offset: g.clone(),
                omega: domain_g.clone(),
                domain_len: domain_size,
                expansion_factor: 2,
                colinearity_test_count: 10,
            },
            &f,
        );

        let mut poly = Polynomial::new(&f);
        poly.term(&BigInt::from(3), 2);
        let mut points: Vec<BigUint> = Vec::new();
        for i in fri.domain() {
            points.push(poly.eval(&i).to_biguint().unwrap());
        }
        fri.prove(&points, &mut channel);
        fri.verify(&mut channel);
    }
}
