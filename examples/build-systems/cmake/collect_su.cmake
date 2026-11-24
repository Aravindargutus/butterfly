# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# -fstack-usage writes each .su beside its object file. CMake puts objects in
# CMakeFiles/<target>.dir/..., which is not where butterfly looks, so copy
# them up to the directory that will be ingested.
#
# Flattening can collide: two sources named alike in different directories
# give two .su of one name, and the second copy would silently replace the
# first -- one unit's frames then fall back to prologue estimates while
# build-info.txt declares -fstack-usage. That stops the build, and so does
# finding none at all (#245).
# IN_LIST, below, needs this in script mode (cmake -P)
cmake_policy(SET CMP0057 NEW)
file(GLOB_RECURSE _su "${FROM}/*.su")
list(LENGTH _su _n)
if(_n EQUAL 0)
  message(FATAL_ERROR "butterfly: -fstack-usage was asked for, but no .su files were written under ${FROM}")
endif()
set(_seen "")
foreach(f ${_su})
  get_filename_component(n "${f}" NAME)
  if(n IN_LIST _seen)
    message(FATAL_ERROR "butterfly: two stack-usage files named ${n} under ${FROM}; refusing to let copy order decide which one is the evidence")
  endif()
  list(APPEND _seen "${n}")
  configure_file("${f}" "${TO}/${n}" COPYONLY)
endforeach()
message(STATUS "butterfly: collected ${_n} .su file(s) into ${TO}")
