# SPDX-FileCopyrightText: 2022 Alyssa Ross <hi@alyssa.is>
# SPDX-License-Identifier: EUPL-1.2

project('hydrasect', 'rust', 'c',
  default_options : ['rust_std=2021', 'warning_level=3'])

c_lib = static_library('hydrasect', 'tmpfd.c')

executable('hydrasect-search', 'hydrasect-search.rs', install : true)

test('Rust unit tests', executable('hydrasect-search-test', 'hydrasect-search.rs',
  link_with : c_lib,
  rust_args : '--test'))
