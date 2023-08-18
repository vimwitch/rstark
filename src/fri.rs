use num_bigint::{Sign, BigUint, BigInt};
use num_bigint::ToBigInt;
use crate::field::Field;
use std::rc::Rc;
use crate::tree::Tree;
use crate::channel::Channel;
use crate::polynomial::Polynomial;

pub struct Fri {
  offset: BigInt,
  omega: BigInt,
  domain_len: u32,
  field: Rc<Field>,
  expansion_factor: u32,
  colinearity_test_count: u32
}

impl Fri {
  fn round_count(&self) -> u32 {
    let mut codeword_len = self.domain_len;
    let mut num_rounds = 0;
    while codeword_len > self.expansion_factor && 4 * self.colinearity_test_count < codeword_len {
      codeword_len /= 2;
      num_rounds += 1;
    }
    num_rounds
  }

  fn eval_domain(&self) -> Vec<BigInt> {
    let mut domain: Vec<BigInt> = Vec::new();
    for i in 0..self.domain_len {
      // TODO: sequential multiplication instead of repeated exp
      domain.push(self.field.mul(&self.offset, &self.field.exp(&self.omega, &BigInt::from(i))));
    }
    domain
  }

  fn prove(&self, codeword: &Vec<BigUint>, channel: & mut Channel) -> Vec<u32> {
    if self.domain_len != codeword.len().try_into().unwrap() {
      panic!("initial codeword does not match domain len");
    }
    let codewords = self.commit(codeword, channel);
    let top_indices = Self::sample_indices(
      &channel.prover_hash(),
      codewords[1].len().try_into().unwrap(),
      codewords[codewords.len() - 1].len().try_into().unwrap(),
      self.colinearity_test_count
    );
    let mut indices: Vec<u32> = top_indices.clone();
    for i in 0..(codewords.len() - 1) {
      indices = indices.iter().map(|index| index % u32::try_from(codewords[i].len() >> 1).unwrap()).collect();
      self.query(&codewords[i], &codewords[i+1], &indices, channel);
    }
    top_indices
  }

  fn query(&self, current_codeword: &Vec<BigUint>, next_codeword: &Vec<BigUint>, indices_c: &Vec<u32>, channel: & mut Channel) {
    let indices_a: Vec<u32> = indices_c.to_vec();
    let indices_b: Vec<u32> = indices_c.iter().map(|val| val + ((current_codeword.len() >> 1) as u32)).collect();
    for i in 0..usize::try_from(self.colinearity_test_count).unwrap() {
      channel.push(&vec!(
        current_codeword[usize::try_from(indices_a[i]).unwrap()].clone(),
        current_codeword[usize::try_from(indices_b[i]).unwrap()].clone(),
        next_codeword[usize::try_from(indices_c[i]).unwrap()].clone()
      ));
    }
    for i in 0..usize::try_from(self.colinearity_test_count).unwrap() {
      channel.push(&Tree::open(indices_a[i], &current_codeword).0);
      channel.push(&Tree::open(indices_b[i], &current_codeword).0);
      channel.push(&Tree::open(indices_c[i], &next_codeword).0);
    }
  }

  fn commit(&self, codeword: &Vec<BigUint>, channel: & mut Channel) -> Vec<Vec<BigUint>> {
    let mut codewords: Vec<Vec<BigUint>> = Vec::new();
    let mut codeword = codeword.clone();
    let mut omega = self.omega.clone();
    let mut offset = self.offset.clone();
    let two_inv = self.field.inv(&BigInt::from(2));
    let zero = BigUint::from(0 as u32);

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
      let alpha = self.field.sample(&channel.prover_hash().to_bigint().unwrap());
      let next_len = codeword.len() >> 1;
      // let mut next_codeword: Vec<BigUint> = vec!(BigUint::from(0 as u32); next_len);
      // next_codeword.clone_from_slice(&codeword[0..next_len]);
      codeword = codeword.iter().enumerate().map(|(index, val)| {
        if index >= next_len {
          return zero.clone();
        }
        let ival = val.to_bigint().unwrap();
        let inv_omega = self.field.inv(&self.field.mul(&offset, &self.field.exp(&omega, &BigInt::from(index))));
        // ( (one + alpha / (offset * (omega^i)) ) * codeword[i]
        let a = self.field.mul(&ival, &self.field.add(&Field::one(), &self.field.mul(&alpha, &inv_omega)));
        //  (one - alpha / (offset * (omega^i)) ) * codeword[len(codeword)//2 + i] ) for i in range(len(codeword)//2)]
        let b = self.field.mul(
          &self.field.sub(&Field::one(), &self.field.mul(&alpha, &inv_omega)),
          &codeword[(codeword.len() >> 1) + index].to_bigint().unwrap()
        );
        return self.field.mul(&two_inv, &self.field.add(&a, &b)).to_biguint().unwrap();
      }).collect();
      codeword.resize(next_len, zero.clone());

      omega = self.field.mul(&omega, &omega);
      offset = self.field.mul(&offset, &offset);
    }
    channel.push(&codeword);
    codewords.push(codeword);
    codewords
  }

  fn sample_indices(seed: &BigUint, size: u32, reduced_size: u32, count: u32) -> Vec<u32> {
    if count > 2*reduced_size {
      panic!("not enough entropy");
    }
    if count > reduced_size {
      panic!("cannot sample more indices than available");
    }

    let mut indices: Vec<u32> = Vec::new();
    // TODO: use a map for this
    let mut reduced_indices: Vec<u32> = Vec::new();
    let mut counter: u32 = 0;
    while indices.len() < (count as usize) {
      let mut hasher = blake3::Hasher::new();
      hasher.update(&seed.to_bytes_le());
      hasher.update(&BigUint::from(counter).to_bytes_le());
      let v = BigInt::from_bytes_le(Sign::Plus, hasher.finalize().as_bytes());
      let index = Field::bigint_to_u32(&(v % BigInt::from(size)));
      let reduced_index = index % reduced_size;
      counter += 1;
      if !reduced_indices.contains(&reduced_index) {
        indices.push(index);
        reduced_indices.push(reduced_index);
      }
    }
    indices
  }

  fn verify(&self, channel: & mut Channel) -> Vec<(BigUint, BigUint)> {
    let mut out: Vec<(BigUint, BigUint)> = Vec::new();
    let mut omega = self.omega.clone();
    let mut offset = self.offset.clone();

    let mut roots: Vec<BigUint> = Vec::new();
    let mut alphas: Vec<BigUint> = Vec::new();

    for _ in 0..self.round_count() {
      roots.push(channel.pull().data[0].clone());
      alphas.push(self.field.sample(&channel.verifier_hash().to_bigint().unwrap()).to_biguint().unwrap());
    }

    let last_codeword = channel.pull().data.clone();
    if roots[roots.len() - 1] != Tree::commit(&last_codeword) {
      panic!("last codeword root mismatch");
    }

    let degree: usize = (last_codeword.len() / usize::try_from(self.expansion_factor).unwrap()) - 1;
    let mut last_omega = omega.clone();
    let mut last_offset = offset.clone();
    for _ in 0..(self.round_count() - 1) {
      last_omega = self.field.mul(&last_omega, &last_omega);
      last_offset = self.field.mul(&last_offset, &last_offset);
    }
    if self.field.inv(&last_omega) != self.field.exp(&last_omega, &BigInt::from(last_codeword.len() - 1)) {
      panic!("omega order incorrect");
    }

    let last_domain = last_codeword.iter().enumerate().map(|(index, _)| {
      return self.field.mul(&last_offset, &self.field.exp(&last_omega, &BigInt::from(index)));
    }).collect();

    let poly = Polynomial::lagrange(&last_domain, &last_codeword.iter().map(|v| v.to_bigint().unwrap()).collect(), &self.field);
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
      self.colinearity_test_count
    );
    for i in 0..usize::try_from(self.round_count() - 1).unwrap() {
      let indices_c: Vec<u32> = top_indices.iter().map(|val| val % (self.domain_len >> (i+1))).collect();
      let indices_a: Vec<u32> = indices_c.clone();
      let indices_b: Vec<u32> = indices_a.iter().map(|val| val + (self.domain_len >> (i+1))).collect();

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
          out.push((BigUint::from(indices_a[j]), y_points_msg.data[0].clone()));
          out.push((BigUint::from(indices_b[j]), y_points_msg.data[1].clone()));
        }

        let ax = self.field.mul(&offset, &self.field.exp(&omega, &BigInt::from(indices_a[j])));
        let bx = self.field.mul(&offset, &self.field.exp(&omega, &BigInt::from(indices_b[j])));

        let cx = alphas[usize::try_from(i).unwrap()].clone();

        if !Polynomial::test_colinearity(&vec!(ax, bx, cx.to_bigint().unwrap()), &vec!(ay.to_bigint().unwrap(), by.to_bigint().unwrap(), cy.to_bigint().unwrap()), &self.field) {
          panic!("colinearity test failed");
        }
      }

      for j in 0..usize::try_from(self.colinearity_test_count).unwrap() {
        Tree::verify(&roots[i], indices_a[j], &channel.pull().data, &aa[j]);
        Tree::verify(&roots[i], indices_b[j], &channel.pull().data, &bb[j]);
        Tree::verify(&roots[i+1], indices_c[j], &channel.pull().data, &cc[j]);
      }

      omega = self.field.mul(&omega, &omega);
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

    let fri = Fri {
      offset: g.clone(),
      omega: domain_g.clone(),
      domain_len: domain_size,
      field: Rc::clone(&f),
      expansion_factor: 2,
      colinearity_test_count: 10
    };

    let mut poly = Polynomial::new(&f);
    poly.term(&BigInt::from(3), 2);
    let mut points: Vec<BigUint> = Vec::new();
    let eval_domain = fri.eval_domain();
    for i in eval_domain {
      points.push(poly.eval(&BigInt::from(i)).to_biguint().unwrap());
    }
    fri.prove(&points, & mut channel);
    fri.verify(& mut channel);
  }
}