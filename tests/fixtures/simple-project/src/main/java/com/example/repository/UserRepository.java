package com.example.repository;

import com.example.model.User;

public interface UserRepository {
    User save(User user);
    User findById(Long id);
    void deleteById(Long id);
}
